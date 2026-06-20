# Scratch Memory

## 定义

scratch memory 是测试辅助内存，不是正式运行时堆。

## 用途

- case runner
- Python stub
- 调试辅助
- 输出回读验证

## 禁止事项

- 不能替代 `malloc`
- 不能替代 `mmap`
- 不能作为正式目标堆
