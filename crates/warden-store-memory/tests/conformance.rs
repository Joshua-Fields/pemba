//! Runs the store conformance suite (`docs/05-storage.md` §9) against `MemoryStore`.

use warden_store_memory::MemoryStore;

#[test]
fn memory_store_conforms() {
    warden_testkit::store_conformance::run(MemoryStore::new);
}
