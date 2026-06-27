//! JNI trampoline hook — 实现 `CodeHook`，在 trampoline 触发时分发 JNI 调用。
//!
//! Guest 调用 `(*env)->FindClass(env, name)` 时：
//! 1. 加载 `env->functions` 获取函数指针表基址
//! 2. 加载 `functions[6]` 获取 FindClass 的 trampoline 地址
//! 3. 跳转到 trampoline 地址
//! 4. CodeHook 在指令执行前触发 → 本模块分派
//!
//! # 架构
//!
//! 本模块持有 `JniFunctionTable`（guest 内存布局）和
//! `Arc<Mutex<AndroidVM>>`（共享 VM 状态）。
//! 每次 trampoline 触发时短暂锁住 VM 来构造 `JniEnvSurface` 并执行 JNI 操作。
//!
//! # NOTE: trampoline + CodeHook 是临时缓解措施
//!
//! 当前 trampoline 页填充 NOP，依赖 Unicorn 的 `add_code_hook` 在指令执行前拦截。
//! 这意味着 trampoline 槽位本身不包含可执行代码——它们只作为"跳转目标"触发 hook。
//! 后续应改为通过 guest 自身的 mmap/allocator 分配 trampoline 区域，
//! 并在槽位中写入真正的跳板指令（如 `BRK` 或 `SVC`），减少对 Unicorn 专有 API 的依赖。
//! 参见 `function_table.rs` 模块文档。

use std::sync::{Arc, Mutex};

use rundroid_backend::{Arm64Reg, CodeHook, GuestCPU};
use rundroid_telemetry::TelemetryEventKind;
use rundroid_jni::{
    function_table::{
        JNI_ALLOC_OBJECT, JNI_CALL_BOOLEAN_METHOD, JNI_CALL_BYTE_METHOD,
        JNI_CALL_CHAR_METHOD, JNI_CALL_DOUBLE_METHOD, JNI_CALL_FLOAT_METHOD,
        JNI_CALL_INT_METHOD, JNI_CALL_LONG_METHOD, JNI_CALL_OBJECT_METHOD,
        JNI_CALL_SHORT_METHOD, JNI_CALL_STATIC_BOOLEAN_METHOD,
        JNI_CALL_STATIC_INT_METHOD, JNI_CALL_STATIC_LONG_METHOD,
        JNI_CALL_STATIC_OBJECT_METHOD, JNI_CALL_STATIC_VOID_METHOD,
        JNI_CALL_VOID_METHOD, JNI_DELETE_GLOBAL_REF, JNI_DELETE_LOCAL_REF,
        JNI_EXCEPTION_CLEAR, JNI_EXCEPTION_OCCURRED, JNI_FIND_CLASS,
        JNI_GET_FIELD_ID, JNI_GET_INT_FIELD, JNI_GET_JAVA_VM, JNI_GET_METHOD_ID,
        JNI_GET_OBJECT_CLASS, JNI_GET_OBJECT_FIELD, JNI_GET_STATIC_FIELD_ID,
        JNI_GET_STATIC_METHOD_ID, JNI_GET_STRING_UTF_CHARS, JNI_GET_VERSION,
        JNI_NEW_GLOBAL_REF, JNI_NEW_LOCAL_REF, JNI_NEW_OBJECT, JNI_NEW_STRING_UTF,
        JNI_REGISTER_NATIVES, JNI_SET_INT_FIELD, JNI_SET_OBJECT_FIELD,
    },
    apply_attach_current_thread, apply_detach_current_thread, apply_get_env,
    validate_jni_version, AndroidVM, JNIEnvABI, JavaVMABI, JavaVMThreadState,
    JniEnvSurface, JniSlotHandler, JNI_ERR, JNI_EDETACHED, JNI_EVERSION, JNI_OK,
    JNI_INVOKE_ATTACH_CURRENT_THREAD, JNI_INVOKE_DETACH_CURRENT_THREAD,
    JNI_INVOKE_GET_ENV,
};
use rundroid_jni::error::JniError;
use rundroid_jni::types::JValue;

/// JNI trampoline 的 CodeHook 实现。
///
/// 持有 guest 可见的 JNIEnv / JavaVM ABI 布局（[`JNIEnvABI`] / [`JavaVMABI`]）、
/// 共享 VM 状态、当前线程 attach 状态，以及 telemetry 收集器。
///
/// [`CodeHook::on_code`] 按 guest PC 落点分流：
/// - 落在 JNIEnv trampoline 区 → [`Self::dispatch_env`]（函数表入口）
/// - 落在 JavaVM trampoline 区 → [`Self::dispatch_invoke`]（invoke table 入口）
/// - 其它（数据区误入）→ fail-fast
pub struct JniTrampolineHook {
    /// JNIEnv ABI 布局（函数表 + trampoline）。
    env_abi: JNIEnvABI,
    /// JavaVM ABI 布局（invoke table + trampoline）。
    vm_abi: JavaVMABI,
    /// 共享 VM 状态。
    vm: Arc<Mutex<AndroidVM>>,
    /// 当前线程 attach 状态（GetEnv/Attach/Detach 状态机）。
    thread_state: JavaVMThreadState,
    /// 共享 telemetry 事件收集器（hook 内写、runtime 读）。
    telemetry: Arc<Mutex<Vec<(String, TelemetryEventKind)>>>,
}

impl JniTrampolineHook {
    /// 构造 trampoline hook。
    ///
    /// 主线程默认已 attach（runtime 在 `JNI_OnLoad` 前 attach 主线程）。
    pub fn new(env_abi: JNIEnvABI, vm_abi: JavaVMABI, vm: Arc<Mutex<AndroidVM>>) -> Self {
        let thread_state = JavaVMThreadState::main_thread(env_abi.env_ptr());
        Self {
            env_abi,
            vm_abi,
            vm,
            thread_state,
            telemetry: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 返回共享的 telemetry 事件收集器引用。
    pub fn telemetry_sink(&self) -> Arc<Mutex<Vec<(String, TelemetryEventKind)>>> {
        Arc::clone(&self.telemetry)
    }

    /// push 一条 telemetry 事件到共享收集器。
    fn emit(&self, name: String, kind: TelemetryEventKind) {
        if let Ok(mut sink) = self.telemetry.lock() {
            sink.push((name, kind));
        }
    }
}

impl CodeHook for JniTrampolineHook {
    fn on_code(&mut self, cpu: &mut dyn GuestCPU, address: u64) {
        // 按地址分流：jni trampoline / javavm trampoline / 异常（数据区误入）。
        let result = if address <= self.env_abi.trampoline_end() {
            self.dispatch_env(cpu, address)
        } else if address >= self.vm_abi.trampoline_begin()
            && address <= self.vm_abi.trampoline_end()
        {
            self.dispatch_invoke(cpu, address)
        } else {
            Err(JniError::Internal(format!(
                "code hook 触发于非 trampoline 地址 0x{address:X}（数据区误入？）"
            )))
        };

        self.finish(cpu, result);
    }
}

impl JniTrampolineHook {
    /// 分派 JNIEnv function table 入口（FindClass / Call*Method / Field / ...）。
    ///
    /// slot 元数据驱动：先查 [`JNIEnvABI::slot_spec`]，未列入或 `Unimplemented` 的
    /// slot 直接 fail-fast 并 telemetry 报名；`Bridge` slot 才进入实际 dispatch。
    fn dispatch_env(&mut self, cpu: &mut dyn GuestCPU, address: u64) -> Result<u64, JniError> {
        let index = self.env_abi.function_index(address);

        // catalog 查询：报名 + 是否已桥接。
        let (slot_name, is_bridge) = match JNIEnvABI::slot_spec(index) {
            Some(spec) => (spec.name, spec.handler == JniSlotHandler::Bridge),
            None => ("unknown", false),
        };
        self.emit(format!("jni.abi_slot.{slot_name}"), TelemetryEventKind::Jni);
        if !is_bridge {
            return Err(JniError::Internal(format!(
                "JNIEnv slot {slot_name} (#{index}) 尚未实现"
            )));
        }

        // 读 JNI 调用参数（x0 = JNIEnv* 由我们管理，从 x1 起为实参）。
        let x1 = cpu.reg_read(Arm64Reg::X(1));
        let x2 = cpu.reg_read(Arm64Reg::X(2));
        let x3 = cpu.reg_read(Arm64Reg::X(3));
        let x4 = cpu.reg_read(Arm64Reg::X(4));
        let x5 = cpu.reg_read(Arm64Reg::X(5));

        // 锁 VM 并构造 JniEnvSurface（字段级 split borrow，互不冲突）。
        let mut vm_guard = self.vm.lock().unwrap();
        let AndroidVM {
            classes,
            objects,
            refs,
            exceptions,
            natives,
            object_id_alloc: _,
            apk: _,
        } = &mut *vm_guard;

        let mut env = JniEnvSurface::new_with_objects(classes, objects, refs, exceptions, natives);

        let mut telemetry_events = Vec::new();
        let result =
            dispatch_jni_call(index, &mut env, cpu, x1, x2, x3, x4, x5, &mut telemetry_events);

        // 合并 dispatch_jni_call 内部产生的事件（如 register_natives）。
        if !telemetry_events.is_empty() {
            if let Ok(mut sink) = self.telemetry.lock() {
                sink.append(&mut telemetry_events);
            }
        }
        result
    }

    /// 分派 JavaVM invoke table 入口（GetEnv / AttachCurrentThread / DetachCurrentThread）。
    ///
    /// 纯状态逻辑（版本校验、attach 状态转换）走 [`JavaVMThreadState`] 系列；
    /// "把 env_ptr 写入 guest 出参 + 返回码写回 x0" 由本方法完成。
    fn dispatch_invoke(
        &mut self,
        cpu: &mut dyn GuestCPU,
        address: u64,
    ) -> Result<u64, JniError> {
        let index = self.vm_abi.function_index(address);

        let slot_name = JavaVMABI::slot_spec(index)
            .map(|s| s.name)
            .unwrap_or("unknown");
        self.emit(format!("jni.abi_slot.{slot_name}"), TelemetryEventKind::Jni);

        // x0 = JavaVM*（由我们管理，忽略）；x1 = 出参 void** / JNIEnv**；x2 = version / args。
        let out_ptr = cpu.reg_read(Arm64Reg::X(1));
        let version = cpu.reg_read(Arm64Reg::X(2));

        match index {
            // GetEnv(JavaVM*, void** env, jint version) → JNI_OK + *env = env_ptr
            JNI_INVOKE_GET_ENV => match apply_get_env(&self.thread_state, version) {
                Ok(env_ptr) => {
                    if !cpu.mem_write(out_ptr, &env_ptr.to_le_bytes()) {
                        return Err(JniError::Internal(format!(
                            "GetEnv: 写 env 出参失败 @ 0x{:X}",
                            out_ptr
                        )));
                    }
                    Ok(JNI_OK)
                }
                Err(_) => {
                    // 区分版本非法 vs 未 attach，映射到对应 JNI 错误码（不静默吞错）。
                    let code = if validate_jni_version(version) {
                        JNI_EDETACHED
                    } else {
                        JNI_EVERSION
                    };
                    self.emit("jni.abi_slot.GetEnv.failed".into(), TelemetryEventKind::Jni);
                    Ok(code)
                }
            },

            // AttachCurrentThread(JavaVM*, JNIEnv** env, void*) → JNI_OK + *env = env_ptr
            JNI_INVOKE_ATTACH_CURRENT_THREAD => {
                let env_ptr = apply_attach_current_thread(&mut self.thread_state)?;
                if !cpu.mem_write(out_ptr, &env_ptr.to_le_bytes()) {
                    return Err(JniError::Internal(
                        "AttachCurrentThread: 写 env 出参失败".into(),
                    ));
                }
                Ok(JNI_OK)
            }

            // DetachCurrentThread(JavaVM*) → JNI_OK
            JNI_INVOKE_DETACH_CURRENT_THREAD => {
                apply_detach_current_thread(&mut self.thread_state)?;
                Ok(JNI_OK)
            }

            _ => Err(JniError::Internal(format!(
                "JavaVM invoke slot {slot_name} (#{index}) 尚未实现"
            ))),
        }
    }

    /// 收尾：写返回值到 x0（错误时写 [`JNI_ERR`]），PC=LR 返回调用者。
    fn finish(&mut self, cpu: &mut dyn GuestCPU, result: Result<u64, JniError>) {
        match result {
            Ok(ret) => {
                cpu.reg_write(Arm64Reg::X(0), ret);
            }
            Err(err) => {
                self.emit(format!("jni.abi_slot.error: {err}"), TelemetryEventKind::Jni);
                cpu.reg_write(Arm64Reg::X(0), JNI_ERR);
            }
        }
        // 跳过 trampoline：设置 PC=LR 返回调用者（code hook 在指令执行前触发）。
        let lr = cpu.reg_read(Arm64Reg::Lr);
        cpu.reg_write(Arm64Reg::Pc, lr);
    }
}

/// 按 JNI 函数索引分派调用。
fn dispatch_jni_call(
    index: usize,
    env: &mut JniEnvSurface<'_>,
    cpu: &mut dyn GuestCPU,
    x1: u64,
    x2: u64,
    x3: u64,
    x4: u64,
    x5: u64,
    telemetry: &mut Vec<(String, TelemetryEventKind)>,
) -> Result<u64, JniError> {
    match index {
        0..=3 => Err(JniError::Internal("JNI reserved slot called".into())),

        JNI_GET_VERSION => {
            Ok(0x0001_0006) // JNI_VERSION_1_6
        }

        JNI_FIND_CLASS => {
            let name = read_cstr_from_guest(cpu, x1)?;
            let handle = env.find_class(&name)?;
            Ok(handle as u64)
        }

        JNI_EXCEPTION_OCCURRED => {
            let has_exception = env.exception_occurred();
            Ok(if has_exception { 1 } else { 0 })
        }

        JNI_EXCEPTION_CLEAR => {
            env.exception_clear();
            Ok(0)
        }

        JNI_NEW_GLOBAL_REF => {
            let handle = x1 as u32;
            let new_handle = env.new_global_ref(handle)?;
            Ok(new_handle as u64)
        }

        JNI_DELETE_GLOBAL_REF => {
            env.delete_global_ref(x1 as u32)?;
            Ok(0)
        }

        JNI_DELETE_LOCAL_REF => {
            env.delete_local_ref(x1 as u32)?;
            Ok(0)
        }

        JNI_NEW_LOCAL_REF => {
            let new_handle = env.new_local_ref_from_handle(x1 as u32)?;
            Ok(new_handle as u64)
        }

        JNI_ALLOC_OBJECT => {
            let handle = env.alloc_object(x1 as u32)?;
            Ok(handle as u64)
        }

        JNI_NEW_OBJECT => {
            let class_handle = x1 as u32;
            let method_id = x2;
            // x3..x5 = constructor args（最多 3 个）
            let args = read_varargs(x3, x4, x5);
            let handle = env.new_object(class_handle, method_id, &args)?;
            Ok(handle as u64)
        }

        JNI_GET_OBJECT_CLASS => {
            let handle = env.get_object_class(x1 as u32)?;
            Ok(handle as u64)
        }

        JNI_GET_METHOD_ID => {
            let class_handle = x1 as u32;
            let method_name = read_cstr_from_guest(cpu, x2)?;
            let method_sig = read_cstr_from_guest(cpu, x3)?;
            let method_id = env.get_method_id(class_handle, &method_name, &method_sig)?;
            Ok(method_id.0)
        }

        JNI_GET_STATIC_METHOD_ID => {
            let class_handle = x1 as u32;
            let method_name = read_cstr_from_guest(cpu, x2)?;
            let method_sig = read_cstr_from_guest(cpu, x3)?;
            let method_id = env.get_static_method_id(class_handle, &method_name, &method_sig)?;
            Ok(method_id.0)
        }

        JNI_GET_FIELD_ID => {
            let class_handle = x1 as u32;
            let field_name = read_cstr_from_guest(cpu, x2)?;
            let field_sig = read_cstr_from_guest(cpu, x3)?;
            let field_id = env.get_field_id(class_handle, &field_name, &field_sig)?;
            Ok(field_id.0)
        }

        JNI_GET_STATIC_FIELD_ID => {
            let class_handle = x1 as u32;
            let field_name = read_cstr_from_guest(cpu, x2)?;
            let field_sig = read_cstr_from_guest(cpu, x3)?;
            let field_id = env.get_static_field_id(class_handle, &field_name, &field_sig)?;
            Ok(field_id.0)
        }

        // —— CallXxxMethod (instance) ——
        JNI_CALL_VOID_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            env.call_void_method_by_id(obj_handle, method_id, &args)?;
            Ok(0)
        }
        JNI_CALL_BOOLEAN_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_boolean_method_by_id(obj_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_BYTE_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_byte_method_by_id(obj_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_CHAR_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_char_method_by_id(obj_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_SHORT_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_short_method_by_id(obj_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_INT_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_int_method_by_id(obj_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_LONG_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_long_method_by_id(obj_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_FLOAT_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_float_method_by_id(obj_handle, method_id, &args)?;
            Ok(v.to_bits() as u64)
        }
        JNI_CALL_DOUBLE_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_double_method_by_id(obj_handle, method_id, &args)?;
            Ok(v.to_bits())
        }
        JNI_CALL_OBJECT_METHOD => {
            let obj_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_object_method(obj_handle, method_id, &args)?;
            Ok(v)
        }

        // —— CallStaticXxxMethod ——
        JNI_CALL_STATIC_VOID_METHOD => {
            let class_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            env.call_static_void_method_by_id(class_handle, method_id, &args)?;
            Ok(0)
        }
        JNI_CALL_STATIC_INT_METHOD => {
            let class_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_static_int_method_by_id(class_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_STATIC_OBJECT_METHOD => {
            let class_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_static_object_method_by_id(class_handle, method_id, &args)?;
            Ok(v)
        }
        JNI_CALL_STATIC_BOOLEAN_METHOD => {
            let class_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_static_boolean_method_by_id(class_handle, method_id, &args)?;
            Ok(v as u64)
        }
        JNI_CALL_STATIC_LONG_METHOD => {
            let class_handle = x1 as u32;
            let method_id = x2;
            let args = read_varargs(x3, x4, x5);
            let v = env.call_static_long_method_by_id(class_handle, method_id, &args)?;
            Ok(v as u64)
        }

        // —— Field get/set ——
        JNI_GET_INT_FIELD => {
            let obj_handle = x1 as u32;
            let field_id = x2;
            let v = env.get_int_field_by_id(obj_handle, field_id)?;
            Ok(v as u64)
        }
        JNI_GET_OBJECT_FIELD => {
            let obj_handle = x1 as u32;
            let field_id = x2;
            let v = env.get_object_field_by_id(obj_handle, field_id)?;
            Ok(v)
        }
        JNI_SET_INT_FIELD => {
            let obj_handle = x1 as u32;
            let field_id = x2;
            let val = x3 as i32;
            env.set_int_field_by_id(obj_handle, field_id, val)?;
            Ok(0)
        }
        JNI_SET_OBJECT_FIELD => {
            let obj_handle = x1 as u32;
            let field_id = x2;
            let val_handle = x3 as u32;
            env.set_object_field_by_id(obj_handle, field_id, val_handle)?;
            Ok(0)
        }

        // —— String ——
        JNI_NEW_STRING_UTF => {
            let utf_str = read_cstr_from_guest(cpu, x1)?;
            let handle = env.new_string_utf(&utf_str)?;
            Ok(handle as u64)
        }
        JNI_GET_STRING_UTF_CHARS => {
            Err(JniError::Internal("GetStringUTFChars 尚未实现".into()))
        }

        // —— RegisterNatives ——
        JNI_REGISTER_NATIVES => {
            let class_handle = x1 as u32;
            let methods_ptr = x2;
            let n_methods = x3 as usize;

            if methods_ptr == 0 || n_methods == 0 {
                return Ok(0);
            }

            // JNINativeMethod 结构 (ARM64, 24 字节):
            //   offset 0: const char* name       (8 字节)
            //   offset 8: const char* signature  (8 字节)
            //   offset 16: void* fnPtr          (8 字节)
            const ENTRY_SIZE: usize = 24;
            let mut parsed = Vec::with_capacity(n_methods.min(256));

            for i in 0..n_methods {
                let entry_addr = methods_ptr + (i * ENTRY_SIZE) as u64;

                let name_ptr = match read_u64_from_guest(cpu, entry_addr) {
                    Some(v) => v,
                    None => continue,
                };
                let sig_ptr = match read_u64_from_guest(cpu, entry_addr + 8) {
                    Some(v) => v,
                    None => continue,
                };
                let fn_ptr = match read_u64_from_guest(cpu, entry_addr + 16) {
                    Some(v) => v,
                    None => continue,
                };

                let name = match read_cstr_from_guest(cpu, name_ptr) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let sig = match read_cstr_from_guest(cpu, sig_ptr) {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                parsed.push((name, sig, fn_ptr));
            }

            let registered = env.register_natives(class_handle, &parsed);
            if registered > 0 {
                telemetry.push(("jni.register_natives".into(), TelemetryEventKind::Jni));
            }
            // JNI 规范：RegisterNatives 返回 0 表示成功，负值表示失败。
            // 注册失败的单个方法被静默跳过（Android linker 容错语义），
            // 只要至少有一个方法注册成功，整体返回成功。
            if registered > 0 {
                Ok(0) // JNI_OK
            } else {
                Ok(0xFFFF_FFFF_FFFF_FFFFu64 as u64) // JNI_ERR (-1)
            }
        }

        // —— GetJavaVM ——
        JNI_GET_JAVA_VM => {
            Err(JniError::Internal("GetJavaVM 尚未接入".into()))
        }

        _ => Err(JniError::Internal(format!("JNI 函数 #{index} 尚未实现"))),
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 从 guest 内存读取一个 u64 值（小端序）。
fn read_u64_from_guest(cpu: &dyn GuestCPU, addr: u64) -> Option<u64> {
    let mut buf = [0u8; 8];
    if cpu.mem_read(addr, &mut buf) {
        Some(u64::from_le_bytes(buf))
    } else {
        None
    }
}

/// 从 guest 内存读取以 NUL 结尾的 C 字符串。
fn read_cstr_from_guest(cpu: &dyn GuestCPU, addr: u64) -> Result<String, JniError> {
    if addr == 0 {
        return Err(JniError::NullNotAllowed("guest C string pointer is NULL".into()));
    }
    let mut buf = Vec::new();
    let mut offset = 0u64;
    while offset < 1024 {
        let mut byte_buf = [0u8; 1];
        if !cpu.mem_read(addr + offset, &mut byte_buf) {
            return Err(JniError::Internal(format!(
                "读取 guest C 字符串失败 @ 0x{:X}", addr + offset
            )));
        }
        if byte_buf[0] == 0 {
            break;
        }
        buf.push(byte_buf[0]);
        offset += 1;
    }
    String::from_utf8(buf)
        .map_err(|_| JniError::Internal("guest C 字符串不是合法 UTF-8".into()))
}

/// 从寄存器读取 varargs 参数（简化处理）。
fn read_varargs(x0: u64, x1: u64, x2: u64) -> Vec<JValue> {
    // 简化：将寄存器值作为 JValue::Long 传递
    // 实际类型由 method handler 根据方法签名解释
    vec![
        JValue::Long(x0 as i64),
        JValue::Long(x1 as i64),
        JValue::Long(x2 as i64),
    ]
}
