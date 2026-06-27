/**
 * jnitest.c — JNI 函数表端到端验证 fixture。
 *
 * 通过 JNIEnv* 函数指针表调用 FindClass → GetStaticMethodID → CallStaticIntMethod 等，
 * 验证 rundroid 的 JNI function table dispatch 链路完整。
 *
 * **重要**：不通过具名 struct 字段访问函数指针表（因为 C struct 布局跳过未声明 slot
 * 时会偏移错误），而是直接用 `((void**)table)[INDEX]` 按索引访问，
 * 确保与 rundroid 的 JNI_TABLE_SIZE=232 / JNI_TRAMPOLINE_SLOT_SIZE=4 对齐。
 *
 * 用法：
 *   aarch64-linux-android21-clang -shared -fPIC -O2 -o libjnitest.so jnitest.c
 */

#include <stdint.h>

/* ---- JNI 类型定义 ---- */
typedef uint32_t jint;
typedef int32_t  jboolean;
typedef void*    jobject;
typedef void*    jclass;
typedef uint64_t jmethodID;

/* ---- JNI 函数表索引（与 rundroid function_table.rs 一致） ---- */
#define JNI_GET_VERSION            4
#define JNI_FIND_CLASS             6
#define JNI_NEW_OBJECT            28
#define JNI_GET_OBJECT_CLASS      31
#define JNI_GET_METHOD_ID         33
#define JNI_CALL_BOOLEAN_METHOD   37
#define JNI_CALL_INT_METHOD       49
#define JNI_CALL_VOID_METHOD      61
#define JNI_GET_STATIC_METHOD_ID 113
#define JNI_CALL_STATIC_BOOLEAN_METHOD 201
#define JNI_CALL_STATIC_INT_METHOD    213
#define JNI_CALL_STATIC_VOID_METHOD   225

/* ---- JNIEnv — 仅首字段（函数指针表） ---- */
typedef struct {
    void** functions;  /* void*[232] */
} JNIEnv;

/* ---- 辅助宏：从函数表按索引提取函数指针并调用 ---- */

/* 从 table 取出索引为 idx 的函数指针，转为 ftype 类型 */
#define JNI_CALL(env, idx, ftype, ...) \
    ((ftype)((env)->functions[idx]))(__VA_ARGS__)

/* ---- 测试函数 ---- */

/**
 * test_get_version:
 *   验证 GetVersion — 最简单的 JNI 函数。
 *   期望返回 JNI_VERSION_1_6 (0x00010006)。
 */
int test_get_version(JNIEnv* env) {
    jint version = JNI_CALL(env, JNI_GET_VERSION, jint (*)(JNIEnv*), env);
    if (version == 0x00010006) {
        return 0;
    }
    return -1;
}

/**
 * test_find_class:
 *   验证 FindClass("test/JniTest") 能返回非空 jclass。
 */
int test_find_class(JNIEnv* env) {
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "test/JniTest");
    if (cls == 0) {
        return -1;
    }
    return 0;
}

/**
 * test_get_static_method_id:
 *   验证 GetStaticMethodID 能找到已注册的 static method。
 */
int test_get_static_method_id(JNIEnv* env) {
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "test/JniTest");
    if (cls == 0) {
        return -1;
    }
    jmethodID mid = JNI_CALL(env, JNI_GET_STATIC_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, cls, "nativePing", "()I");
    if (mid == 0) {
        return -2;
    }
    return 0;
}

/**
 * test_call_static_int_method:
 *   验证 CallStaticIntMethod 能调用已注册的 static method 并拿到返回值。
 *   Rust 侧 nativePing()I 返回 42。
 */
int test_call_static_int_method(JNIEnv* env) {
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "test/JniTest");
    if (cls == 0) {
        return -1;
    }
    jmethodID mid = JNI_CALL(env, JNI_GET_STATIC_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, cls, "nativePing", "()I");
    if (mid == 0) {
        return -2;
    }
    jint result = JNI_CALL(env, JNI_CALL_STATIC_INT_METHOD,
        jint (*)(JNIEnv*, jclass, jmethodID),
        env, cls, mid);
    return result;
}

/**
 * test_call_void_method:
 *   验证 CallVoidMethod — 完整的 FindClass → GetMethodID(ctor) → NewObject → CallVoidMethod 链路。
 */
int test_call_void_method(JNIEnv* env) {
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "test/JniTest");
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
    jmethodID mid = JNI_CALL(env, JNI_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, cls, "doNothing", "()V");
    if (mid == 0) {
        return -4;
    }
    JNI_CALL(env, JNI_CALL_VOID_METHOD,
        void (*)(JNIEnv*, jobject, jmethodID),
        env, obj, mid);
    return 0;
}

/**
 * jni_full_flow:
 *   完整链路：FindClass → GetMethodID(ctor) → NewObject → GetMethodID(getAndIncrement) → CallIntMethod×2
 *   class: test/Counter
 *     - instance method getAndIncrement()I → 返回当前值后 +1（Arc<SharedField> 实现）
 *   Rust 侧初始 count=100。
 *
 *   第一次 getAndIncrement 返回 100（count 变 101）
 *   第二次 getAndIncrement 返回 101（count 变 102）
 *   返回 (r1 << 16) | r2
 */
int jni_full_flow(JNIEnv* env) {
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "test/Counter");
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
    jmethodID mid = JNI_CALL(env, JNI_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, cls, "getAndIncrement", "()I");
    if (mid == 0) {
        return -4;
    }
    jint r1 = JNI_CALL(env, JNI_CALL_INT_METHOD,
        jint (*)(JNIEnv*, jobject, jmethodID),
        env, obj, mid);
    jint r2 = JNI_CALL(env, JNI_CALL_INT_METHOD,
        jint (*)(JNIEnv*, jobject, jmethodID),
        env, obj, mid);

    if (r1 != 100) {
        return -10 - (r1 & 0xFF);
    }
    if (r2 != 101) {
        return -20 - (r2 & 0xFF);
    }
    return (r1 << 16) | (r2 & 0xFFFF);
}

/* ============================================================
 * JavaVM invoke table 验证（rundroid jni-abi-surfaces）
 *
 * guest 通过 `(*vm)->GetEnv(vm, &env, version)` 从 JavaVM invoke table
 * 取 JNIEnv*，验证 JavaVMABI 的 GetEnv/AttachCurrentThread 入口。
 * ============================================================ */

/* ---- JavaVM — 仅首字段（invoke 函数指针表） ---- */
typedef struct {
    void** functions;  /* JNIInvokeInterface* (void*[8]) */
} JavaVM;

/* ---- JavaVM invoke table 索引（与 rundroid abi.rs JNI_INVOKE_* 一致）----
 * JNIInvokeInterface: 前 3 槽 reserved NULL，随后
 *   DestroyJavaVM=3, AttachCurrentThread=4, DetachCurrentThread=5, GetEnv=6
 */
#define JNI_INVOKE_ATTACH_CURRENT_THREAD 4
#define JNI_INVOKE_DETACH_CURRENT_THREAD 5
#define JNI_INVOKE_GET_ENV 6

/* 从 invoke table 按索引取函数指针并调用 */
#define JNI_VM_CALL(vm, idx, ftype, ...) \
    ((ftype)(((JavaVM*)(vm))->functions[idx]))(__VA_ARGS__)

/**
 * test_get_env_via_javavm:
 *   验证通过 JavaVM invoke table 调 GetEnv 能拿到有效 JNIEnv*。
 *   GetEnv(vm, &env, JNI_VERSION_1_6) → 返回 JNI_OK(0)、env 非空；
 *   再用该 env 调 GetVersion + FindClass 验证 env 对当前 VM 有效。
 *
 *   返回 0 表示成功；负值表示各阶段失败码。
 */
int test_get_env_via_javavm(JavaVM* vm) {
    JNIEnv* env = 0;
    jint ret = JNI_VM_CALL(vm, JNI_INVOKE_GET_ENV,
        jint (*)(JavaVM*, void**, jint),
        vm, (void**)&env, 0x00010006);
    if (ret != 0) {
        return -100 - (ret & 0x7F);  /* GetEnv 返回非 JNI_OK */
    }
    if (env == 0) {
        return -1;
    }
    /* 用 GetEnv 返回的 env 调 GetVersion，验证 env 对当前 VM 有效 */
    jint version = JNI_CALL(env, JNI_GET_VERSION, jint (*)(JNIEnv*), env);
    if (version != 0x00010006) {
        return -2;
    }
    /* 再 FindClass 进一步验证 env 完整可用 */
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "test/JniTest");
    if (cls == 0) {
        return -3;
    }
    return 0;
}

/**
 * test_attach_via_javavm:
 *   验证 JavaVM invoke table 的 AttachCurrentThread / DetachCurrentThread 端到端
 *   （覆盖 GetEnv 之外的另两个 invoke 入口）。
 *
 *   1. AttachCurrentThread(vm, &env, NULL) → JNI_OK + env 非空
 *   2. 用 attach 返回的 env 调 FindClass 验证 env 有效
 *   3. DetachCurrentThread(vm) → JNI_OK
 *
 *   返回 0 表示成功；负值表示各阶段失败码。
 */
int test_attach_via_javavm(JavaVM* vm) {
    JNIEnv* env = 0;
    jint ret = JNI_VM_CALL(vm, JNI_INVOKE_ATTACH_CURRENT_THREAD,
        jint (*)(JavaVM*, void**, void*),
        vm, (void**)&env, (void*)0);
    if (ret != 0) {
        return -100 - (ret & 0x7F);  /* AttachCurrentThread 返回非 JNI_OK */
    }
    if (env == 0) {
        return -1;
    }
    /* 用 attach 返回的 env 调 FindClass 验证 env 有效 */
    jclass cls = JNI_CALL(env, JNI_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*),
        env, "test/JniTest");
    if (cls == 0) {
        return -2;
    }
    /* DetachCurrentThread(JavaVM*) → JNI_OK */
    jint ret2 = JNI_VM_CALL(vm, JNI_INVOKE_DETACH_CURRENT_THREAD,
        jint (*)(JavaVM*), vm);
    if (ret2 != 0) {
        return -300 - (ret2 & 0x7F);
    }
    return 0;
}
