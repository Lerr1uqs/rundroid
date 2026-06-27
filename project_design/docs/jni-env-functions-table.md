
functions table 所有fnptr 第 i 格指向 trampoline_base + i*4 (全是nop)
```rust
        // 3. invoke trampoline 页：全部 NOP
        for i in 0..JNI_INVOKE_TABLE_SIZE {
            let tramp = self.trampoline_base + (i as u64) * TRAMPOLINE_SLOT_SIZE;
            mem_write(tramp, &ARM64_NOP);
        }
```

未来也可以采用 svc的方式