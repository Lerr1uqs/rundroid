## 1. Python MRO 收集修正

- [x] 1.1 修改 `python/rundroid/javashim/base.py` 中 `JavaClass.__init_subclass__` 的去重逻辑，按 Java descriptor 而不是 Python 方法名判定覆写
- [x] 1.2 确保 `__java_methods__` 与 `__java_dispatch__` 共用同一份 descriptor 覆写结果，不出现一边过滤一边保留的分叉

## 2. 回归测试

- [x] 2.1 在 `python/tests/test_javaclass_call.py` 新增“父类与子类不同 Python 名、相同 descriptor”的注册成功与调用命中测试
- [x] 2.2 增加“子类覆写一个 descriptor 时，其他不同 descriptor 的重载仍保留”的测试

## 3. 验证

- [x] 3.1 在 `python/` 项目上下文运行相关 pytest 用例，确认新覆写场景通过且既有 javaclass-call 测试不回归
- [x] 3.2 运行 `openspec validate --type change python-javaclass-mro-descriptor-override --strict`
