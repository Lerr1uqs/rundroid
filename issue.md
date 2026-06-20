# 审计出来的问题
›     fn reg_read(&self, reg: Arm64Reg) -> u64 {
          match translate_reg(reg) {
              Ok(r) => self.uc.reg_read(r).unwrap_or(0),
              Err(_) => 0,
          }
      }

  另外这里是不是兜底太过了？传错不应该报错吗？这部分也在spec


  