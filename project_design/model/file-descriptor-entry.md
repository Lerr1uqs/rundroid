# FileDescriptorEntry

## 定义

`FileDescriptorEntry` 是 `FileDescriptorTable` 中的单个描述符槽位。

## 最小组成

- `fd`
- `kind`
- `handle_ref`
- `descriptor_flags`
- `virtual_path` 可选，仅用于路径来源对象的观测与诊断

## 关键语义

- 它不是 regular file、device、socket 或 pipe 本体
- 它引用一个已打开 handle；per-open 状态保存在 handle 中
- descriptor 级元数据放在 `FileDescriptorEntry`
- `dup/dup2/dup3` 会生成新的 `FileDescriptorEntry`，并按既定策略共享或克隆 handle
- non-path 对象如 socket、pipe、eventfd 也应进入同一张表
