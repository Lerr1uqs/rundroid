# ELF

## 定义

ELF 子系统拆成三层：

- `parser`
- `loader`
- `linker`

## parser

负责格式读取与只读抽象：

- headers
- dynamic
- `soname`
- `needed`
- relocation 归一化

## loader

负责单模块装入：

- 地址空间保留
- 段映射
- 写入与零填充
- 形成模块对象

## linker

负责模块图级别连接：

- 依赖顺序
- 符号解析
- relocation 写回
- init 顺序
- `RELRO` 协作
