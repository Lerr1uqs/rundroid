# FileDescriptorTable

## 定义

`FileDescriptorTable` 是 `OS` 持有的 fd 句柄表。

它把整数 fd 映射到 `FileDescriptorEntry`，统一承接 `open`、`socket`、`pipe`、`eventfd` 等来源。

## 负责什么

- 分配、查询、替换、移除 `FileDescriptorEntry`
- 保证 syscall 先经由 fd 查到条目，再分发到已打开 handle
- 统一收纳有路径对象和无路径对象

## 不负责什么

- 路径挂载
- 设备注册
- 直接实现 `read/write/ioctl/mmap`

## 关键语义

- 它是句柄表，不是路径表
- 一个 fd 对应一个 `FileDescriptorEntry`
- `close` 删除条目后，fd 才能被重用
- `dup/dup2/dup3` 创建新的 `FileDescriptorEntry`，而不是重走路径解析
