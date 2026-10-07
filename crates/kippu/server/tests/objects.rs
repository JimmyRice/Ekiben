//! Choosing an object storage adapter from `images.url`.
#![allow(clippy::unwrap_used, reason = "test: a failure is the test failing")]

use kippu_server::adapters::connect_objects;

#[test]
fn only_object_stores_are_accepted() {
    for local in ["file:///var/lib/kippu/images", "memory:///"] {
        let error = connect_objects(local, &[]).unwrap_err().to_string();
        assert!(error.contains("stateless"), "{error}");
    }
    let error = connect_objects("ftp://example.org/x", &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("no object storage adapter"), "{error}");
}

#[cfg(feature = "s3")]
#[test]
fn provider_settings_are_checked() {
    let region = vec![("aws_region".to_owned(), "eu-west-1".to_owned())];
    connect_objects("s3://kippu-images/prod", &region).unwrap();
    let unknown = vec![("not_a_setting".to_owned(), "x".to_owned())];
    let error = connect_objects("s3://kippu-images/prod", &unknown)
        .unwrap_err()
        .to_string();
    assert!(error.contains("not_a_setting"), "{error}");
    assert!(connect_objects("s3://", &[]).is_err());
}
