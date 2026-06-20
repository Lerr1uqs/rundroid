# VFS

## 定义

VFS 是“虚拟路径到节点语义”的映射层。

当前阶段采用显式挂载表，不强制 rootfs。

## 核心挂载面

- `map_file(virtual_path, ...)`
- `map_device(virtual_path, ...)`

## 节点类型

- 普通文件节点
- 设备节点
- 动态 provider 节点
