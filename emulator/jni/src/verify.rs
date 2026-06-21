//! Python 注解与 Java descriptor 严格匹配校验。
//!
//! 在 Python bridge 将 decorated method / field 注册到 Rust runtime 之前，
//! 通过此模块做 exact match 检查。不匹配在注册阶段直接失败，
//! 错误信息包含 class / member / descriptor / 期望类型 / 实际类型。

use crate::error::JniError;
use crate::types::{JType, MethodSig};

/// 从 Python bridge 传入的注解元数据。
///
/// Python decorator 声明的参数类型和返回值类型被提取为 `PythonCallableAnnotations`，
/// 然后在注册阶段与 Java descriptor 解析结果做严格匹配。
#[derive(Debug, Clone)]
pub struct PythonCallableAnnotations {
    /// 返回值类型。
    pub return_type: JType,
    /// 参数类型列表（按顺序）。
    pub param_types: Vec<JType>,
}

impl PythonCallableAnnotations {
    /// 创建新的注解元数据。
    pub fn new(return_type: JType, param_types: Vec<JType>) -> Self {
        Self { return_type, param_types }
    }

    /// 校验 method 的 Python 注解与 Java method descriptor 是否 exact match。
    ///
    /// # 检查项
    ///
    /// 1. 参数数量一致
    /// 2. 每个参数类型完全匹配（包括 Object 的 class name）
    /// 3. 返回值类型完全匹配
    ///
    /// # 返回值
    ///
    /// 匹配成功返回 `Ok(())`，否则返回 `JniError::VerifyFailed`
    /// 携带 class name、method name、descriptor、期望值和实际值的完整信息。
    pub fn verify(&self, sig: &MethodSig) -> Result<(), JniError> {
        // 检查参数数量
        if self.param_types.len() != sig.args.len() {
            return Err(JniError::VerifyFailed {
                class_name: sig.class.clone(),
                member: sig.name.clone(),
                descriptor: sig.to_string(),
                expected: format!("{} 个参数", sig.args.len()),
                actual: format!("{} 个参数", self.param_types.len()),
            });
        }

        // 逐参数检查类型匹配
        for (i, (ann_ty, sig_ty)) in self.param_types.iter().zip(sig.args.iter()).enumerate() {
            if ann_ty != sig_ty {
                return Err(JniError::VerifyFailed {
                    class_name: sig.class.clone(),
                    member: sig.name.clone(),
                    descriptor: sig.to_string(),
                    expected: format!("第 {i} 参数: {sig_ty:?}"),
                    actual: format!("第 {i} 参数: {ann_ty:?}"),
                });
            }
        }

        // 检查返回值类型
        if self.return_type != sig.ret {
            return Err(JniError::VerifyFailed {
                class_name: sig.class.clone(),
                member: sig.name.clone(),
                descriptor: sig.to_string(),
                expected: format!("返回值: {:?}", sig.ret),
                actual: format!("返回值: {:?}", self.return_type),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MethodSig;

    fn make_sig() -> MethodSig {
        MethodSig::parse("TestClass.hashCode()I").unwrap()
    }

    fn make_sig_with_args() -> MethodSig {
        MethodSig::parse("TestClass.foo(ILjava/lang/String;)V").unwrap()
    }

    fn make_sig_single_arg() -> MethodSig {
        MethodSig::parse("TestClass.foo(I)V").unwrap()
    }

    #[test]
    fn verify_match_succeeds() {
        let sig = make_sig();
        let annotations = PythonCallableAnnotations::new(JType::Int, vec![]);
        assert!(annotations.verify(&sig).is_ok());
    }

    #[test]
    fn verify_return_type_mismatch_fails() {
        let sig = make_sig();
        let annotations = PythonCallableAnnotations::new(JType::Long, vec![]);
        let err = annotations.verify(&sig).unwrap_err();
        assert!(matches!(err, JniError::VerifyFailed { .. }));
    }

    #[test]
    fn verify_param_count_mismatch_fails() {
        let sig = make_sig_with_args();
        let annotations = PythonCallableAnnotations::new(JType::Void, vec![]);
        let err = annotations.verify(&sig).unwrap_err();
        assert!(matches!(err, JniError::VerifyFailed { .. }));
    }

    #[test]
    fn verify_param_type_mismatch_fails() {
        let sig = make_sig_single_arg();
        let annotations = PythonCallableAnnotations::new(JType::Void, vec![JType::Long]);
        let err = annotations.verify(&sig).unwrap_err();
        assert!(matches!(err, JniError::VerifyFailed { .. }));
    }
}
