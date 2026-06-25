Java 的基础类型（primitive types）在 JNI 中不需要创建 jobject，它们直接以 C/C++ 的标量值形式传递。

| Java 类型   | JNI 类型     | C/C++ 类型         | 需要创建对象？ |
| --------- | ---------- | ---------------- | ------- |
| `byte`    | `jbyte`    | `signed char`    | ❌ 不需要   |
| `short`   | `jshort`   | `short`          | ❌ 不需要   |
| `int`     | `jint`     | `int`            | ❌ 不需要   |
| `long`    | `jlong`    | `long long`      | ❌ 不需要   |
| `float`   | `jfloat`   | `float`          | ❌ 不需要   |
| `double`  | `jdouble`  | `double`         | ❌ 不需要   |
| `boolean` | `jboolean` | `unsigned char`  | ❌ 不需要   |
| `char`    | `jchar`    | `unsigned short` | ❌ 不需要   |


什么时候需要object
| 场景                 | 处理方式                                               |
| ------------------ | -------------------------------------------------- |
| 返回 `int` 给 Java    | 直接 `return 42;`                                    |
| 返回 `String` 给 Java | 需要 `NewStringUTF` 创建 `jstring`（它是 `jobject` 的引用类型） |
| 返回自定义对象            | 需要 `AllocObject` + 设置字段，或 `NewObject` 调用构造函数       |
| 数组（`int[]` 等）      | 用 `NewIntArray` 创建 `jintArray`（它是 `jobject`）       |
