# Resolver

## 定义

`Resolver` 是“根据模块标识找到真实模块内容”的能力。

## 负责什么

- 解析 `module_uri`
- 为 root module 提供字节来源
- 为 `DT_NEEDED` 提供递归定位能力

## 不负责什么

- ELF 解析
- 内存映射
- 符号解析
