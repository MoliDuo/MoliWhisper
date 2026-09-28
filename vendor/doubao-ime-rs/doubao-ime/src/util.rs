use rand::Rng;

/// 生成 16 位随机数字作为匿名设备标识。
pub fn gen_device_id() -> String {
    rand::thread_rng()
        .gen_range(1_000_000_000_000_000u64..10_000_000_000_000_000)
        .to_string()
}
