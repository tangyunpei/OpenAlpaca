//! ST-19 step 3 on the service: deleting a project's data drops every
//! owner's partition of it from the index and removes its control
//! snapshots, while the route holds the project's slot — so this must not
//! take one. The note files are the folder's, removed by the route after.

use super::tests::{ALICE, BOB, Harness};
use super::*;
use openalpaca_storage::memory_files::{NoteKind, Partition};
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::store::projects::{self, Project};

fn project(h: &Harness, name: &str) -> Project {
    projects::register(&h.workspace(name, Some(".git"))).unwrap()
}

fn key(owner: &str, scope: StoreScope) -> String {
    Partition::open(&scope, owner)
        .unwrap()
        .partition_key()
        .to_string()
}

#[test]
fn removing_a_project_drops_every_owners_partition_and_no_other() {
    let h = Harness::new();
    let p = project(&h, "p");
    let q = project(&h, "q");
    let body = "# A note\n\nSomething worth keeping.\n";
    let alice_p = h.store(
        &h.user(ALICE, Some(p.root.as_path())),
        "Alice in P",
        body,
        NoteKind::Fact,
    );
    let bob_p = h.store(
        &h.user(BOB, Some(p.root.as_path())),
        "Bob in P",
        body,
        NoteKind::Fact,
    );
    let alice_q = h.store(
        &h.user(ALICE, Some(q.root.as_path())),
        "Alice in Q",
        body,
        NoteKind::Fact,
    );
    let home = h.store(
        &h.user(ALICE, None),
        "Alice at home",
        body,
        NoteKind::Preference,
    );
    let alice_p_key = key(ALICE, StoreScope::Project(p.root.clone()));
    let bob_p_key = key(BOB, StoreScope::Project(p.root.clone()));
    let alice_q_key = key(ALICE, StoreScope::Project(q.root.clone()));
    let home_key = key(ALICE, StoreScope::Home);

    // The route holds the project's slot for the whole delete.
    let _held = h.service.quiesce(&p.id).unwrap();
    assert_eq!(h.service.remove_project(&p.id).unwrap(), 2);

    let indexed =
        |key: &str, id: &NoteId| h.service.index.get(key, &id.to_string()).unwrap().is_some();
    assert!(!indexed(&alice_p_key, &alice_p.id));
    assert!(!indexed(&bob_p_key, &bob_p.id));
    assert!(indexed(&alice_q_key, &alice_q.id));
    assert!(indexed(&home_key, &home.id));

    let keys: Vec<String> = files::list_snapshots()
        .unwrap()
        .into_iter()
        .filter_map(|(_, parsed)| parsed.ok())
        .map(|s| s.partition_key)
        .collect();
    assert!(!keys.contains(&alice_p_key));
    assert!(!keys.contains(&bob_p_key));
    assert!(keys.contains(&alice_q_key));
    assert!(keys.contains(&home_key));

    // The note files go with the folder, not with this call.
    assert!(p.dir.join("memory").join(files::OWNERS_DIR).is_dir());
    assert_eq!(
        h.service.remove_project(&p.id).unwrap(),
        0,
        "nothing left to drop"
    );
}
