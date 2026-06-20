# VirtFile

## 定义

`VirtFile` 是普通文件节点的统一抽象。

## 来源

- `VirtFile.host(...)`
- `VirtFile.bytes(...)`
- 动态文件 provider

## 关键语义

- 它是文件，不是设备
- 成功读取必须落实到目标缓冲区
- 不能“拿到源字节就算成功”
