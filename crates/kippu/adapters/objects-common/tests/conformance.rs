//! The bridge, proven with an in-memory `object_store`.

use std::sync::Arc;

use kippu_objects_common::ObjectStoreAdapter;
use kippu_store::ObjectStorage;
use object_store::memory::InMemory;

kippu_store::object_conformance_tests!(async {
    Some(Arc::new(ObjectStoreAdapter::new(Arc::new(InMemory::new()))) as Arc<dyn ObjectStorage>)
});
