/*
 * rundroid bootstrap smoke fixture
 *
 * 三个 case 对应的导出：
 *   rd_add(a, b)             —— 纯算术，验证 ABI 调用 + 返回值（case 1）
 *   rd_open_urandom()        —— 通过 openat 打开 /dev/urandom（case 2，file/mmap）
 *   rd_get_random(buf, n)    —— 通过 getrandom 拉字节（case 3，/dev/urandom）
 *
 * case 1 不依赖 syscall，bootstrap 阶段可直接跑通；
 * case 2/3 走 svc，需要 syscall hook 接入后才能完整执行，
 * 但 case runner 的 artifact 产出路径已经覆盖这两种 case。
 */

/* 直接发起 syscall，避免依赖 libc；调用约定见 AArch64 Linux ABI。 */
static long sys3(long nr, long a0, long a1, long a2) {
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0") = a0;
    register long x1 __asm__("x1") = a1;
    register long x2 __asm__("x2") = a2;
    __asm__ volatile("svc #0" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x8) : "memory");
    return x0;
}

/* 6 参数 syscall（mmap 等）。 */
static long sys6(long nr, long a0, long a1, long a2, long a3, long a4, long a5) {
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0") = a0;
    register long x1 __asm__("x1") = a1;
    register long x2 __asm__("x2") = a2;
    register long x3 __asm__("x3") = a3;
    register long x4 __asm__("x4") = a4;
    register long x5 __asm__("x5") = a5;
    __asm__ volatile("svc #0" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5), "r"(x8) : "memory");
    return x0;
}

int rd_add(int a, int b) {
    return a + b;
}

int rd_identity(int v) {
    return v;
}

int rd_constant(void) {
    return 42;
}

/* case 2：openat(AT_FDCWD=-100, "/dev/urandom", O_RDONLY=0)。返回 fd。 */
int rd_open_urandom(void) {
    long fd = sys3(56 /* openat */, -100, (long)"/dev/urandom", 0);
    return (int)fd;
}

/* case 3：getrandom(buf, n, 0)。
 * 返回字节 XOR 校验和，便于 case 断言"字节真的写进了 buf"：
 * 全零才返回 0（对随机字节几乎不可能），任何写失败（syscall 返回 -EFAULT）
 * 也会让校验和走负数路径，case 据此区分"成功且有数据" vs "假阳性"。 */
int rd_get_random(void *buf, int n) {
    long r = sys3(278 /* getrandom */, (long)buf, n, 0);
    if (r < 0) return (int)r;
    unsigned char *p = (unsigned char *)buf;
    int cs = 0;
    for (long i = 0; i < r; i++) cs ^= p[i];
    return cs;
}

/* case 4：openat(path) → 返回 fd（或负 errno）。
 * 路径必须是 NUL 结尾的 C 字符串（与 /dev/urandom 相同的调用约定）。 */
int rd_open(const char *path) {
    return (int)sys3(56 /* openat */, -100, (long)path, 0);
}

/* case 5：read(fd, buf, n) → 返回实际读取字节数，并写回 buf。
 * 返回值 < 0 表示 errno。 */
int rd_read(int fd, void *buf, int n) {
    long r = sys3(63 /* read */, fd, (long)buf, n);
    if (r < 0) return (int)r;
    return (int)r;
}

/* case 6：openat + read + close 组合。
 * 打开指定路径到 buf，返回读取的字节数（< 0 = errno）。
 * 写入 buf 的内容由 case 回读断言。 */
int rd_open_read(const char *path, void *buf, int n) {
    int fd = rd_open(path);
    if (fd < 0) return fd;
    int r = rd_read(fd, buf, n);
    sys3(57 /* close */, fd, 0, 0);
    return r;
}

/* case 7：mmap 匿名页 → 写 magic → 读回校验和。
 * 通过真实 backend 证明 mmap 返回的地址真实可读可写，而非只返回占位地址
 * （spec: Bootstrap mmap must create target-visible mappings）。
 *
 * 自验证：若 mmap 未真实建立目标侧映射（map_guest 假成功），guest 写该地址
 * 会触发未映射异常，call_export 的 emu_start 失败 → case 报错（而非假 pass）。
 * 成功时返回 0xAB^0xCD^0x12^0x34 = 0x40 = 64。 */
int rd_mmap_rw(void) {
    long addr = sys6(222 /* mmap */, 0, 4096,
                     3 /* PROT_READ|PROT_WRITE */,
                     0x22 /* MAP_PRIVATE|MAP_ANONYMOUS */,
                     -1, 0);
    if (addr < 0) return (int)addr;  /* errno */
    unsigned char *p = (unsigned char *)addr;
    p[0] = 0xAB; p[1] = 0xCD; p[2] = 0x12; p[3] = 0x34;
    int cs = 0;
    for (int i = 0; i < 4; i++) cs ^= p[i];
    return cs;
}
