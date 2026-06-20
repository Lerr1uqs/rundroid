# Python Stub

## 定义

Python stub 是脚本扩展层，不是核心运行时。

## 负责什么

- 声明设备类
- 声明文件 provider
- 编写 case 行为
- 快速扩展测试逻辑

## 不负责什么

- 持有最终目标内存状态
- 持有 fd table
- 持有 mount table 真正状态

## 注册原则

- 可以显式 `map_file` / `map_device`
- 可以用 decorator 声明默认虚拟路径
- 真正生效仍需显式注册
