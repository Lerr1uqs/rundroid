
可以通过两种渠道注册 java class 来完成补环境

一种是外部python script
```python
@java_class("android/content/pm/Signature")
class Signature(JavaClass):

	def __init__(self):
		# 正常进行初始化 复制成员函数 这个Signature会被实例化的
		self._msig = bytes([])

	@java_method("Signature([B)V") # 构造函数
	def signature_init(self, sig) -> JVoid:
		self._msig = sig

	@java_method("hashCode()I")
    def hash_code(self) -> JInt:
    	pass

    @java_field(name="mSignature", sig="[B") # 应该是这么描述field？ or @java_field("mSignature[B")
    def member_signature(self) -> JArray[JByte]:
    	return self._msig
```

另外一种是builtin rust注册
参考 `experiment/`

两种注册方式 最终都把JavaClassMeta注册给AndroidRuntime/AVM 去做统一管理

另外 有时候需要在Python层做 javaclass的生成 + 调用 比如我这里 JavaClass某一个函数 设置了一个 native函数 调用对应的so 那么我在python构造执行的同时 就能执行native的模拟逻辑
这个和unidbg的设计是一致的

所以也许需要 JavaClass 定义析构函数 能够将自己的销毁情况传递回rust-core

