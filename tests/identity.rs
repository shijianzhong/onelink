//! endpoint 身份密钥持久化：同一路径每次读到的身份一致、损坏时报错而不是换身份、
//! 并发首次创建不会互相踩坏文件。

use onelink::net::load_or_create_secret_key;

#[test]
fn same_path_yields_same_identity_across_calls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("endpoint.key");

    let first = load_or_create_secret_key(&path).expect("首次应生成密钥");
    let second = load_or_create_secret_key(&path).expect("再次应读取同一密钥");

    assert_eq!(
        first.public().to_string(),
        second.public().to_string(),
        "重启后 node id 必须不变，否则对端会多出一台新设备"
    );
    assert_eq!(std::fs::read(&path).unwrap().len(), 32);
}

#[test]
fn corrupt_key_file_is_an_error_not_a_silent_new_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("endpoint.key");
    std::fs::write(&path, b"broken").unwrap();

    let err = load_or_create_secret_key(&path).expect_err("损坏的密钥文件必须报错");
    assert!(
        err.to_string().contains("长度不对"),
        "错误信息应说明原因: {err}"
    );
    // 关键：不能悄悄换身份——文件必须原样保留，等用户决定怎么处理。
    assert_eq!(std::fs::read(&path).unwrap(), b"broken");
}

#[test]
fn concurrent_first_creation_converges_on_one_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("endpoint.key");

    // 8 个线程同时首次创建：hard_link 竞态下也只能留下一把钥匙。
    let mut handles = Vec::new();
    for _ in 0..8 {
        let path = path.clone();
        handles.push(std::thread::spawn(move || {
            load_or_create_secret_key(&path)
                .expect("并发创建也不该失败")
                .public()
                .to_string()
        }));
    }
    let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let first = &ids[0];
    assert!(
        ids.iter().all(|id| id == first),
        "并发创建产生了多个身份: {ids:?}"
    );
    let on_disk = load_or_create_secret_key(&path)
        .unwrap()
        .public()
        .to_string();
    assert_eq!(&on_disk, first);
}

#[cfg(unix)]
#[test]
fn key_file_is_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("endpoint.key");

    load_or_create_secret_key(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "密钥文件权限应为 0600，实际 {mode:o}");
}
