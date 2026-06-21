
可以通过两种渠道注册 java class 来完成补环境

一种是外部python script
```python
@java_class("android/content/pm/Signature")
class Signature(JavaObject):

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

有个情况我也说明一下 就是实际上一般来说不需要在python中去实例化这个对象(JavaObject) 实例化一般是在rust层 python只负责定义+注册
然后执行native的时候rust层会从定义部分去调用对应的函数 目前python层的创建(new_java_instance)更多是为了调试+测试

