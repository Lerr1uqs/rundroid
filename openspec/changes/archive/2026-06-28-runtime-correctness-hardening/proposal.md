## Why

`bootstrap-runtime` 已经把 `rundroid` 的最小执行主线搭起来了，但当前主线还停留在“能跑 smoke case”的阶段，尚未达到“结果可信、可作为后续 JNI/hook/driver 基座”的阶段。

现有实现已经暴露出几类典型问题：

- syscall 可以在目标内存写入失败时仍然返回成功
- `DT_NEEDED` 依赖图没有真正接通
- 页权限和 RELRO 目前只是概念存在，未真正作用到 backend
- case manifest 的 `arch` / `backend` / `seed` 等关键字段未真正生效
- 某些 case 名义上覆盖 `mmap` / `/dev/urandom`，但真实断言强度不足

如果在这些问题未收敛前继续扩展 JNI、hook、driver 或多 backend，会把错误语义固化到更高层，后面返工成本会明显变高。

## What Changes

这个 change 用来把 bootstrap runtime 从“可运行”推进到“结果可信”。

本次变更引入：

- 目标内存可见性的强约束
- `DT_NEEDED` / `DT_SONAME` 驱动的最小依赖装载与链接语义
- 页权限与 RELRO 的最小正确性路径
- case manifest 关键字段的强制生效
- 更可信的 smoke / regression case
- 以 `unidbg` 为参考的自然语言实现说明，帮助 worker-agent 直接落地

本次变更不要求：

- 完整 JNI
- 完整 TLS 运行时
- ARM32/Thumb
- 完整驱动模拟
- GDB/LLDB 全功能接入
- host bridge / driver bridge 全量能力

## Capabilities

这个 change 会新增或定义：

- runtime-correctness
- dependency-linking
- testing-harness

## Impact

实现方在完成本 change 之前，不应继续把主要精力放在 JNI、hook、driver 广度扩张上。

review 阶段应优先看：

- 目标内存是否真的写入成功
- 链接是否按依赖图而不是扫描所有模块
- 页权限是否真的收紧
- case 是否真的断言了目标侧行为
