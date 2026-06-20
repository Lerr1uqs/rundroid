# Backend

## 定义

`Backend` 是最底层执行抽象。

它统一表达：

- 寄存器访问
- 目标内存访问
- PC 控制
- 停止执行
- 页权限调整

## 负责什么

- 执行 ARM64 指令
- 读写寄存器
- 读写目标内存
- 建立与收紧页权限

## 不负责什么

- syscall 语义
- ELF 依赖解析
- 文件系统路径解释
- Python 设备注册

## 当前实现方向

当前阶段优先使用 Unicorn 作为 `Backend` 实现。  
但 `Backend` 这个模型本身不能绑定到 Unicorn API。
