//! The adapter against a real bucket.
//!
//! Set `EKIBEN_TEST_GCS_URL` (e.g. `gs://kippu-images/production`) and the provider's credentials to run these;
//! without it they are skipped. Each run writes under its own key prefix.

use std::sync::Arc;

use kippu_store::ObjectStorage;

kippu_store::object_conformance_tests!(async {
    let url = std::env::var("EKIBEN_TEST_GCS_URL").ok()?;
    let objects = kippu_objects_gcs::connect(&url, &[]).unwrap();
    Some(Arc::new(objects) as Arc<dyn ObjectStorage>)
});
