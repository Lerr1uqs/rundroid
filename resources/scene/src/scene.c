/**
 * scene.c — python-jni-execution Phase 4 的 rich 集成压测 fixture。
 *
 * 目的：在最小 fixture（jnimin）打通主链后，压测真实 Android 逆向场景下
 * JNI dispatch / 继承 / primitive 参数 marshalling / syscall / verbose 的交叉正确性。
 *
 * 覆盖（对应 testing-harness spec「Rich native-.so scene」）：
 *   - JNI_OnLoad + RegisterNatives
 *   - 跨多 class 的 FindClass / GetMethodID / CallIntMethod（instance）
 *     + GetStaticMethodID / CallStaticIntMethod（static）+ 继承 + 交叉依赖
 *   - syscall：mmap / getrandom / openat / read（inline svc，不依赖 libc）
 *   - 一个 checksum / hash 算法（27-bit finalizer + 交叉混合）
 *
 * # guest 侧 Java 类（由 Python 测试注册为 @java_class shim）
 *   - com/scene/Signer  —— hash(I)I instance（hash finalizer）
 *   - com/scene/Verifier extends Signer —— 继承 hash + 自有 nonce()I
 *   - com/scene/Crypto  —— mix(II)I static（交叉混合）
 *   - com/scene/Scene   —— verifyNative(I)I native（RegisterNatives 绑定目标）
 *
 * # marshalling 边界
 * trampoline 的 read_varargs 把寄存器值统一读成 JValue::Long，Python 侧 jvalue_to_py
 * 还原成 int —— 故 guest→Python 方向只支持 primitive（int/long）入参。本 fixture 的
 * Python 方法签名全部用 int 入参/返回（str/byte[] 经 guest varargs 的编组是已知边界，
 * 由 python-jni-value-marshalling 的方向 B / call_java_method_typed 覆盖）。
 *
 * # guest-created 对象与 self=None
 * guest 经 NewObject 创建的对象在 Rust 侧是 StubInstance（无 Python backing），
 * instance override 经 wrap_python_method 注入 self=None 后以纯计算形式调用——
 * 故 Signer.hash / Verifier.nonce 都是不依赖 self 的纯函数。
 *
 * # 确定性返回值
 * 成功返回 27-bit 正 int（[0, 0x07FFFFFF)）；任一 syscall 失败或继承解析失败返回
 * 独立的正 int 哨兵（0x400000xx，< 0x7FFFFFFF，保证 jint 正值、x0 无符号歧义）。
 * Python 测试用同款算法复算 expected 并断言相等——相等即转证 syscall + 继承 + 交叉调用全绿。
 *
 * 用法：
 *   aarch64-linux-android21-clang -shared -fPIC -O2 -o libscene.so scene.c
 */

#include <stdint.h>

/* ---- JNI 类型定义（不引入 <jni.h>） ---- */
typedef uint32_t jint;
typedef void*    jobject;
typedef void*    jclass;
typedef uint64_t jmethodID;

#define JNI_VERSION_1_6 0x00010006

#define JNI_EXPORT extern
#define JNIEXPORT  JNI_EXPORT
#define JNICALL

/* ---- JNIEnv / JavaVM 函数表索引（与 rundroid function_table.rs 对齐） ----
 * 不通过具名 struct 字段访问函数指针表（C struct 布局跳过未声明 slot 会偏移错误），
 * 直接按索引 ((void**)table)[INDEX] 访问。 */
#define SCENE_FIND_CLASS              6
#define SCENE_NEW_OBJECT             28
#define SCENE_GET_METHOD_ID          33
#define SCENE_GET_STATIC_METHOD_ID  113
#define SCENE_CALL_INT_METHOD        49
#define SCENE_CALL_STATIC_INT_METHOD 213
#define SCENE_REGISTER_NATIVES      215

/* JavaVM invoke table：前 3 槽 reserved，GetEnv=6 */
#define SCENE_INVOKE_GET_ENV          6

/* ---- JNIEnv / JavaVM —— 仅首字段（函数指针表） ---- */
typedef struct { void** functions; } JNIEnv;
typedef struct { void** functions; } JavaVM;

/* 从函数表按索引取函数指针并调用。Call*Method 带 varargs 的用「...」声明 ftype，
 * 让编译器把多余实参放进 x3..x5（trampoline 的 read_varargs 从那里读）。 */
#define ENV_CALL(env, idx, ftype, ...) ((ftype)((env)->functions[idx]))(__VA_ARGS__)
#define VM_CALL(vm, idx, ftype, ...)   ((ftype)(((JavaVM*)(vm))->functions[idx]))(__VA_ARGS__)

/* ============================================================
 * raw syscall —— inline svc，不依赖 libc 动态符号
 *（rundroid ELF loader 不提供 bionic，故 mmap/openat/read 等不能走 libc 包装）。
 * ARM64 Linux 约定：syscall 号在 x8，参数 x0..x5，返回 x0。
 * ============================================================ */
static long scene_syscall(long nr, long a0, long a1, long a2, long a3, long a4, long a5) {
    register long x8 asm("x8") = nr;
    register long x0 asm("x0") = a0;
    register long x1 asm("x1") = a1;
    register long x2 asm("x2") = a2;
    register long x3 asm("x3") = a3;
    register long x4 asm("x4") = a4;
    register long x5 asm("x5") = a5;
    asm volatile("svc #0"
        : "+r"(x0)
        : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5)
        : "memory", "cc");
    return x0;
}

/* ARM64 Linux syscall 号（rundroid runtime subset） */
#define SYS_SCENE_MMAP      222
#define SYS_SCENE_GETRANDOM 278
#define SYS_SCENE_OPENAT     56
#define SYS_SCENE_READ       63
#define SYS_SCENE_CLOSE      57

#define SCENE_PROT_RW       3              /* PROT_READ | PROT_WRITE */
#define SCENE_MAP_PRIV_ANON 0x22           /* MAP_PRIVATE(0x2) | MAP_ANONYMOUS(0x20) */
#define SCENE_AT_FDCWD      (-100)

/* 成功结果 < 0x08000000；失败哨兵 0x400000xx（均 < 0x7FFFFFFF，jint 正值） */
#define ERR_MMAP        0x40000001
#define ERR_GETRANDOM   0x40000002
#define ERR_OPENAT      0x40000003
#define ERR_READ        0x40000004
#define ERR_CLASS       0x40000006        /* FindClass / GetMethodID 解析失败 */
#define ERR_INHERIT     0x40000005        /* 继承解析破坏（inh_hash != sig_hash） */

/* JNINativeMethod 结构（ARM64, 24 字节）：name / signature / fnPtr 各 8 字节 */
typedef struct {
    const char* name;
    const char* signature;
    void*       fnPtr;
} JNINativeMethod;

/* com/scene/Scene.verifyNative 的 guest native 实现（RegisterNatives 绑定目标）。
 * bootstrap 不支持经 JNI 表分派 guest native（嵌套 emu_start 未接入），故此函数注册后
 * 不被经表调用；其地址仅作 RegisterNatives 的 fnPtr 占位，证明绑定链路打通。 */
static jint scene_verify_native_stub(JNIEnv* env, jint input) {
    (void)env;
    (void)input;
    return 0;
}

/* ============================================================
 * 主入口：syscall 覆盖 + 跨 class JNI（instance + static + 继承 + 交叉）+ checksum
 *   返回 27-bit 确定性 hash（成功）或 0x400000xx 哨兵（失败）。
 * ============================================================ */
JNIEXPORT jint JNICALL
Java_com_scene_Native_run(JNIEnv* env, jint input) {
    /* ===== syscall 覆盖：mmap / getrandom / openat / read ===== */

    /* mmap 匿名映射一页缓冲 */
    long buf = scene_syscall(SYS_SCENE_MMAP, 0, 0x1000,
                             SCENE_PROT_RW, SCENE_MAP_PRIV_ANON, -1, 0);
    if (buf < 0) {
        return ERR_MMAP;
    }
    /* getrandom 往映射缓冲写 4 字节（不经 fd，直接 syscall） */
    if (scene_syscall(SYS_SCENE_GETRANDOM, buf, 4, 0, 0, 0, 0) != 4) {
        return ERR_GETRANDOM;
    }
    /* openat + read /dev/urandom（builtin 自动挂载） */
    long fd = scene_syscall(SYS_SCENE_OPENAT, SCENE_AT_FDCWD,
                            (long)"/dev/urandom", 0, 0, 0, 0);
    if (fd < 0) {
        return ERR_OPENAT;
    }
    char ub[4];
    long n = scene_syscall(SYS_SCENE_READ, fd, (long)ub, 4, 0, 0, 0);
    scene_syscall(SYS_SCENE_CLOSE, fd, 0, 0, 0, 0, 0);
    if (n != 4) {
        return ERR_READ;
    }

    /* ===== 跨 class instance：Signer.hash(input) ===== */
    jclass signer_cls = ENV_CALL(env, SCENE_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*), env, "com/scene/Signer");
    if (!signer_cls) {
        return ERR_CLASS;
    }
    jmethodID sctor = ENV_CALL(env, SCENE_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, signer_cls, "<init>", "()V");
    jobject signer = ENV_CALL(env, SCENE_NEW_OBJECT,
        jobject (*)(JNIEnv*, jclass, jmethodID), env, signer_cls, sctor);
    jmethodID hash_mid = ENV_CALL(env, SCENE_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, signer_cls, "hash", "(I)I");
    int sig_hash = ENV_CALL(env, SCENE_CALL_INT_METHOD,
        jint (*)(JNIEnv*, jobject, jmethodID, ...), env, signer, hash_mid, input);

    /* ===== 继承：Verifier extends Signer —— 继承的 hash + 自有 nonce ===== */
    jclass ver_cls = ENV_CALL(env, SCENE_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*), env, "com/scene/Verifier");
    if (!ver_cls) {
        return ERR_CLASS;
    }
    jmethodID vctor = ENV_CALL(env, SCENE_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, ver_cls, "<init>", "()V");
    jobject ver = ENV_CALL(env, SCENE_NEW_OBJECT,
        jobject (*)(JNIEnv*, jclass, jmethodID), env, ver_cls, vctor);
    /* 继承关键：对子类 Verifier 取 "hash" 的 method id，应沿 superclass 链解析到 Signer */
    jmethodID ihash_mid = ENV_CALL(env, SCENE_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, ver_cls, "hash", "(I)I");
    int inh_hash = ENV_CALL(env, SCENE_CALL_INT_METHOD,
        jint (*)(JNIEnv*, jobject, jmethodID, ...), env, ver, ihash_mid, input);
    if (inh_hash != sig_hash) {
        return ERR_INHERIT;  /* 继承解析破坏：子类调用与父类结果不一致 */
    }
    jmethodID nonce_mid = ENV_CALL(env, SCENE_GET_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, ver_cls, "nonce", "()I");
    int vnonce = ENV_CALL(env, SCENE_CALL_INT_METHOD,
        jint (*)(JNIEnv*, jobject, jmethodID, ...), env, ver, nonce_mid);

    /* ===== 跨 class static：Crypto.mix(sig_hash, vnonce) ===== */
    jclass crypto_cls = ENV_CALL(env, SCENE_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*), env, "com/scene/Crypto");
    if (!crypto_cls) {
        return ERR_CLASS;
    }
    jmethodID mix_mid = ENV_CALL(env, SCENE_GET_STATIC_METHOD_ID,
        jmethodID (*)(JNIEnv*, jclass, const char*, const char*),
        env, crypto_cls, "mix", "(II)I");
    int mixed = ENV_CALL(env, SCENE_CALL_STATIC_INT_METHOD,
        jint (*)(JNIEnv*, jclass, jmethodID, ...), env, crypto_cls, mix_mid,
        sig_hash, vnonce);

    /* ===== checksum 算法：混合三方结果成 27-bit 确定性 hash =====
     * unsigned 运算避免 signed overflow UB；& 0x07FFFFFF 保证正值、与 Python 复算一致。 */
    unsigned combine = (unsigned)mixed
                     ^ ((unsigned)inh_hash * 31u)
                     ^ (unsigned)input;
    return (jint)(combine & 0x07FFFFFF);
}

/* ============================================================
 * JNI_OnLoad —— GetEnv + RegisterNatives(com/scene/Scene.verifyNative)
 * ============================================================ */
JNIEXPORT jint JNICALL
JNI_OnLoad(JavaVM* vm, void* reserved) {
    (void)reserved;
    JNIEnv* env = 0;
    jint ret = VM_CALL(vm, SCENE_INVOKE_GET_ENV,
        jint (*)(JavaVM*, void**, jint), vm, (void**)&env, JNI_VERSION_1_6);
    if (ret != 0 || env == 0) {
        return -1;
    }

    /* RegisterNatives：把 com/scene/Scene.verifyNative 绑定到 guest native stub。
     * com/scene/Scene 由 Python 测试注册（含 verifyNative(I)I 声明），RegisterNatives
     * 据其 MethodId 落 NativeRegistry 绑定（count=1 → JNI_OK），证明绑定链路打通。 */
    jclass scene_cls = ENV_CALL(env, SCENE_FIND_CLASS,
        jclass (*)(JNIEnv*, const char*), env, "com/scene/Scene");
    if (scene_cls) {
        JNINativeMethod m = { "verifyNative", "(I)I", (void*)scene_verify_native_stub };
        ENV_CALL(env, SCENE_REGISTER_NATIVES,
            jint (*)(JNIEnv*, jclass, const JNINativeMethod*, jint),
            env, scene_cls, &m, 1);
    }
    return JNI_VERSION_1_6;
}
