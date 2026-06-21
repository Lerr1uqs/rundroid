JavaEnv 在 JNI 中代表 JNI 接口指针表，是 Java 虚拟机暴露给原生代码（C/C++）的核心函数调用接口。

| 能力             | 典型函数                                         | 说明                      |
| -------------- | -------------------------------------------- | ----------------------- |
| **访问/修改字段**    | `GetObjectField` / `SetIntField`             | 绕过 Java 访问控制            |
| **调用 Java 方法** | `CallVoidMethod` / `CallStaticObjectMethod`  | 从 native 回调 Java        |
| **字符串转换**      | `NewStringUTF` / `GetStringUTFChars`         | Java String ↔ C char\*  |
| **数组操作**       | `GetByteArrayElements` / `SetIntArrayRegion` | 原始类型数组访问                |
| **对象操作**       | `NewObject` / `FindClass` / `GetMethodID`    | 在 native 层创建/查找 Java 对象 |
| **异常处理**       | `ExceptionCheck` / `ExceptionClear`          | 检查并清理 Java 异常           |
| **局部引用管理**     | `NewLocalRef` / `DeleteLocalRef`             | 防局部引用表溢出（512 条目上限）      |


获取方式
```c
JavaVM* vm;
(*env)->GetJavaVM(env, &vm);           // 缓存全局 JavaVM*
vm->AttachCurrentThread(&env, NULL);   // 其他线程获取 JNIEnv*
```

引用相关api
```c
// 局部引用
jobject   NewLocalRef(JNIEnv*, jobject);
void      DeleteLocalRef(JNIEnv*, jobject);
jint      EnsureLocalCapacity(JNIEnv*, jint capacity);  // 预扩容
jint      PushLocalFrame(JNIEnv*, jint capacity);        // 批量作用域
jobject   PopLocalFrame(JNIEnv*, jobject result);        // 批量释放

// 全局引用
jobject   NewGlobalRef(JNIEnv*, jobject);
void      DeleteGlobalRef(JNIEnv*, jobject);
jweak     NewWeakGlobalRef(JNIEnv*, jobject);
void      DeleteWeakGlobalRef(JNIEnv*, jobject);
jboolean  IsSameObject(JNIEnv*, jobject, jobject);      // 弱引用判空专用
```

使用方式: 
1. 缓存cls和method 注意这里拿到的是cls不是instance
```c++
// 假设 Java 层：
// public class Signature {
//     public byte[] sign(byte[] data) { ... }
// }

// ① JNI_OnLoad 缓存类和方法 ID（只做一次）
static jclass g_sig_cls;      // GlobalRef
static jmethodID g_sign_mid;  // 实例方法 ID

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
    JNIEnv* env;
    (*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6);
    
    jclass local = (*env)->FindClass(env, "com/example/Signature");
    g_sig_cls = (*env)->NewGlobalRef(env, local);
    (*env)->DeleteLocalRef(env, local);
    
    // 实例方法用 GetMethodID，签名 (参数)返回值
    g_sign_mid = (*env)->GetMethodID(env, g_sig_cls, "sign", "([B)[B");
    return JNI_VERSION_1_6;
}

// ② Native 层创建实例并调用
JNIEXPORT jbyteArray JNICALL native_sign(
    JNIEnv* env, jobject thiz, 
    jobject sig_instance,      // Java 传下来的已有实例
    jbyteArray data
) {
    // 方式 A：Java 已有实例传下来，直接调用
    jbyteArray result = (*env)->CallObjectMethod(
        env, 
        sig_instance,          // jobject：实例引用
        g_sign_mid,            // jmethodID
        data                   // 参数
    );
    
    // 方式 B：Native 层自己 new 实例（需要构造函数）
    jmethodID ctor = (*env)->GetMethodID(env, g_sig_cls, "<init>", "()V");
    jobject new_sig = (*env)->NewObject(env, g_sig_cls, ctor);
    
    jbyteArray result2 = (*env)->CallObjectMethod(env, new_sig, g_sign_mid, data);
    
    (*env)->DeleteLocalRef(env, new_sig);  // 释放临时实例
    return result2;
}

```

所以模拟 JNIEnv 需要做 class管理 ref管理 localframe管理 处理外部的call method