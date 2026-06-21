# 审计出来的问题

- [x] reg_read / reg_write 兜底过当：translate_reg 失败时不应该返回 0 / 静默跳过，
      已改为 panic（let-it-failed）。unicorn reg_read/reg_write 失败也改为 panic。
- [x] SyscallCpu 改名 HookCpu：它在 hook 上下文中提供 CPU 寄存器/内存操作，
      不局限于 syscall。贯穿 backend api / unicorn / case-runner / bindings。
