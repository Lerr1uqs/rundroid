# module_uri

## 定义

`module_uri` 是模块来源定位符。

它描述“模块从哪里读取”，不是显示名。

## 示例

- `resource:cases/so/libfoo.so`
- `file:F:/samples/libfoo.so`
- `apk:/lib/arm64-v8a/libfoo.so`

## 作用

- 定位 root module
- 定位 `DT_NEEDED` 依赖
- 统一 resource / file / apk 入口
