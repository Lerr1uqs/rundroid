# VirtualDevice

## 定义

`VirtualDevice` 是设备节点抽象。

## 关键语义

- 通过虚拟路径挂载
- `open` 后生成 per-fd 实例
- 后续行为按 fd 分发

## 最小行为面

- `open`
- `read`
- `write`
- `ioctl`
- `mmap`
- `fstat`
- `close`
