# OS

## 定义

`OS` 是建立在 `Backend` 之上的运行语义层。

如果说 `Backend` 解决“怎么执行”，  
那么 `OS` 解决“这些执行在 Linux/Android 语义上代表什么”。

## 负责什么

- syscall 分发
- fd 生命周期
- 地址空间协作
- `read` / `pread` / `getrandom`
- `mmap`
- manifest 参数生效

## 不负责什么

- ELF 原始字节解析
- Python 类声明
- case 文件格式定义
