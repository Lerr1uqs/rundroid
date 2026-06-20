# ModuleGraph

## 定义

`ModuleGraph` 表示模块依赖图，不是简单模块列表。

## 必须表达的关系

- root module
- direct dependency
- dependency closure
- 稳定遍历顺序

## 存在意义

它用于保证：

- `DT_NEEDED` 真正参与装载
- 符号解析顺序稳定
- 结果不依赖全局扫描
- 结果不依赖 `HashMap` 遍历偶然性
