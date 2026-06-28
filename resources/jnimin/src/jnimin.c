/**
 * jnimin.c — Python 绑定层 JNI execution surface 的最小验证 fixture。
 *
 * 目的：把故障面收窄到「ABI 表映射 / hook 安装 / env-vm 指针传递 / 基础 dispatch」，
 * 覆盖最小 JNI 主链：JNI_OnLoad → FindClass → GetMethodID → NewObject → CallIntMethod。
 * 一旦此 fixture 不绿，就不应继续调 rich scene（resources/scene）。
 *
 * 调用的 Java 侧（com/jnimin/Counter）由 Python 测试注册为 @java_class shim：
 *   - <init>()V   —— 构造器（NewObject 必需的 method id）
 *   - getValue()I —— Python override，返回常量（纯计算，验证 guest→Python 回调）
 *
 * **重要**：与 jnitest.c 一致，不通过具名 struct 字段访问函数指针表
 * （C struct 布局跳过未声明 slot 时会偏移错误），而是直接用
 * `((void**)table)[INDEX]` 按索引访问，与 rundroid function_table.rs 对齐。
 *
 * 用法：
 *   aarch64-linux-android21-clang -shared -fPIC -O2 -o libjnimin.so jnimin.c
 */

#include <stdint.h>

/* ---- JNI 类型定义 ---- */
typedef uint32_t jint;
typedef void*    jobject;
typedef void*    jclass;
typedef uint64_t jmethodID;

#define JNI_VERSION_1_6 0x00010006

/* JNIEXPORT / JNICALL：本 fixture 不引入 <jni.h>，此处给空定义
 *（-fPIC -shared 默认可见性已导出非 static 符号）。 */
#define JNI_EXPORT	extern
#define JNIEXPORT	JNI_EXPORT
#define JNICALL

/* ---- JNIEnv 函数表索引（与 rundroid function_table.rs 一致） ---- */
#define JNI_FIND_CLASS       6
#define JNI_NEW_OBJECT      28
#define JNI_GET_METHOD_ID   33
#define JNI_CALL_INT_METHOD 49

/* ---- JNIEnv — 仅首字段（函数指针表） ---- */
typedef struct {
    void** functions;  /* void*[232] */
} JNIEnv;

/* 从函数表按索引取函数指针并调用 */
#define JNI_CALL(env, idx, ftype, ...) \
    ((ftype)((env)->functions[idx]))(__VA_ARGS__)

/* ============================================================
 * 主入口：完整最小 JNI 主链
 *   FindClass → GetMethodID(<init>) → NewObject
 *            → GetMethodID(getValue) → CallIntMethod
 *   返回 CallIntMethod 的结果（com/jnimin/Counter.getValue 的返回值）。
 *   各阶段失败返回不同负值，便于定位 surface 断点。
 * ============================================================ */
JNIEXPORT jint JNICALL
Java_com_jnimin_Native_run(JNIEnv* env) {
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "com/jnimin/Counter");
    if (cls == 0) {
        return -1;
    }

    jmethodID ctor = JNI_CALL(env, JNI_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, cls, "<init>", "()V");
    if (ctor == 0) {
        return -2;
    }

    jobject obj = JNI_CALL(env, JNI_NEW_OBJECT,
        jobject (*)(JNIEnv*, jclass, jmethodID),
        env, cls, ctor);
    if (obj == 0) {
        return -3;
    }

    jmethodID getValue = JNI_CALL(env, JNI_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, cls, "getValue", "()I");
    if (getValue == 0) {
        return -4;
    }

    return JNI_CALL(env, JNI_CALL_INT_METHOD,
        jint (*)(JNIEnv*, jobject, jmethodID),
        env, obj, getValue);
}

/* ============================================================
 * JavaVM invoke table（rundroid jni-abi-surfaces）
 * ============================================================ */

/* ---- JavaVM — 仅首字段（invoke 函数指针表） ---- */
typedef struct {
    void** functions;  /* JNIInvokeInterface* (void*[8]) */
} JavaVM;

/* JNIInvokeInterface: 前 3 槽 reserved NULL，随后 GetEnv=6 */
#define JNI_INVOKE_GET_ENV 6

#define JNI_VM_CALL(vm, idx, ftype, ...) \
    ((ftype)(((JavaVM*)(vm))->functions[idx]))(__VA_ARGS__)

/**
 * JNI_OnLoad：模块装载后由绑定层 jni_onload() 自动调用。
 *   GetEnv(vm, &env, JNI_VERSION_1_6) → 校验 JavaVM 已接、env 可用；
 *   返回合法 JNI version（绑定层 validate_jni_version 校验，非法 fail-fast）。
 */
JNIEXPORT jint JNICALL
JNI_OnLoad(JavaVM* vm, void* reserved) {
    (void)reserved;
    JNIEnv* env = 0;
    jint ret = JNI_VM_CALL(vm, JNI_INVOKE_GET_ENV,
        jint (*)(JavaVM*, void**, jint),
        vm, (void**)&env, JNI_VERSION_1_6);
    if (ret != 0 || env == 0) {
        return -1;
    }
    return JNI_VERSION_1_6;
}
