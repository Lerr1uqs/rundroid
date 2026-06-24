handle = jobject，就是一个"引用票根"，有生命周期（Local/Global）
ObjectId = 对象在JVM堆里的真实身份

```c++
// handle = jobject
// ObjectId = 对象本身（你无法直接看到，它在JVM堆里）

// 函数1：Java传进来一个对象
JNIEXPORT void JNICALL Java_Foo_save(JNIEnv* env, jobject thiz, jobject obj) {
    // obj 是一个 handle（Local引用）
    // 它指向 JVM 堆里的某个对象（ObjectId）
    
    // NewGlobalRef：同一个对象，拿到一个新的 handle（Global）
    jobject global = env->NewGlobalRef(obj);
    
    // 现在：同一个对象，有两个 handle
    // obj = Local handle（栈帧结束自动失效）
    // global = Global handle（永久有效，直到 DeleteGlobalRef）
    
    env->DeleteLocalRef(obj);  // 删除 Local handle，对象还在，因为 global 还在
}
```