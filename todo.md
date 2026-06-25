- @java_method 中 需不需要进行static声明？

54 +### 3. `method_names` 仍然以 `(class_name, java_method_name)` 为 key，会破坏 overload 语义
55 +
56 +JNI foundation 的更上游规范要求 method key 使用完整 `MethodSig`，不能只靠方法名，否则 overload 无法稳定区分。
57 +
58 +但当前 adapter 仍然把 Python 映射存成：
59 +
60 +- `(class_name, java_method_name) -> python_method_name`
61 +
62 +插入和查询都只用 `sig.name`，不包含 descriptor。
63 +
64 +这意味着同一 class 下如果出现：
65 +
66 +- `foo(I)I`
67 +- `foo(Ljava/lang/String;)I`
68 +
69 +当前 direct-Python 路径无法正确区分，最后一个注册项会覆盖前一个映射。
70 +
71 +证据：
72 +
73 +- `openspec/changes/jni-shim-foundation/specs/jni-shim/spec.md:74-75`
74 +- `emulator/bindings/python/src/lib.rs:217-261`
75 +- `emulator/bindings/python/src/lib.rs:574-579`
76 +- `emulator/bindings/python/src/lib.rs:797-809`
77 +
78 +这不只是“未来优化项”，而是当前 adapter 设计已经和上游 typed-signature 约束冲突。


- java primitive type到py type的转换？

- JavaClass 是否要添加一个release函数 手动释放