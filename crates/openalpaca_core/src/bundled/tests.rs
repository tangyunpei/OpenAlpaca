//! The bundle's guards (subagent design §2.2 V-3, §9): the list knows every
//! bundled file, never loses a line, and was generated from the versions the
//! fixtures hold.

use super::test_support::{step_7, step_8};
use super::*;
use crate::orchestrator::MAIN_LOOP_AGENT_ID;
use std::path::{Path, PathBuf};

/// The 109 lines the list held when C3 wrote it: the 100 versions the
/// repository's history holds of the 17 bundled paths (both sides of
/// TEMPLATE-TIMEOUT among them, V-2b) and the nine results of the
/// Installation Manual's steps 7 and 8 (V-2a). A frozen floor: the list may
/// grow, never lose one of these.
const FLOOR: &str = "\
36a2c2939a96ac35113a1fdcb012115b5101262837300cd645adbbf5a09e6fb6 agents/code_agent.md
385573c7652a391e31a5cc95a58be4d2cd3d13275690db78a6c91518955108cc agents/code_agent.md
65b2bcf7f063df131546c371cc143664cb1eb0fc1094522fd5d379f36f31fdd6 agents/code_agent.md
adf7c06fb4432ea65c6a426b271a96c41da44136d7dafe7adb2063cdf7ca6463 agents/code_agent.md
c4fe2ad144acff0b8feb3f6ec47c1249b0ad9c507fd307c510dd1a4fe6ecb052 agents/code_agent.md
d42f306081a075da6d68c13b7823cdfc19a2528601d13c27ea521d4ddbf16358 agents/code_agent.md
d6063a620033e6288e148f386dde3090ab1cfc450248039d8222063c6d9a97de agents/code_agent.md
1e83b592d707b8b5d0fee9a124697470e5e1373cbab3a788ce0b073662327206 agents/explore_agent.md
40cb224398efe1c39e622663b842f8fa06182329d20f656ba523e8999f602e35 agents/explore_agent.md
4777ea1cf3b7fff70855523d31c8c15db2ae49885d9a77016e237e962716928f agents/explore_agent.md
6c45d783c265e70764118839f538edb3b5b44ff3dc236c97b7627c36536148ea agents/explore_agent.md
9032e286887172026db2957bee6ea752f30d899842f27c020db39f0ddfd657c9 agents/explore_agent.md
939483001237352b24743d7a58d258eb26d258bf1865b391da7665fb74114d39 agents/explore_agent.md
a14fa635368c0affd2eda046a84df7815cb4494a134ac292f7911a97cd705c69 agents/explore_agent.md
c25d013b17b1cca0fce11c05a4521f44109cdda9797107dfa31c600e75bb1983 agents/explore_agent.md
09efadf062f03fbc79b019447d9e6e3eafa97a85191584ba3faef775f377f098 agents/general_agent.md
322acc67ea5b2233c14bb5f1325c921895e643fd4f4c8d705b9c80fa24752534 agents/general_agent.md
5c8f486ed94a69b6a8863f1f4ea149c6f1386b8af7a95463c0833c08a476ddc5 agents/general_agent.md
6c4dfde4035dc845f4b08cde1291a1d3b5239210a9f606b3f7f854a76fe7871a agents/general_agent.md
70721be5101726361b1fbe995c3fbfcd9b385ef65c735e9d20ba36b096cfd852 agents/general_agent.md
79381f10a260215b2f12eb77f01c479b3c89a52c8c1240496ad905747397a992 agents/general_agent.md
7c01809b2c8dfff85f533b1d27cb39fac838d5b1a64f8d561947c4c089ca986e agents/general_agent.md
a549c690d407f541e0c5b5f4819271c56f19284c6e06a0e4fe927691c7053461 agents/general_agent.md
a9ef167608062783100076878b17477f4ff800b7133e7c2d8451d376361bee6a agents/general_agent.md
e3cace126531ff1fc1f4dbb8269a8acd98e5feedd5a416d831718508ccbaa9e5 agents/general_agent.md
0938a473b0c4ce471965153289e5fe16438fc56c4854130e4cb00befda015714 agents/lead_agent.md
50d636f76eceb02fb51bfe1d69da890bf9c13e591fb8ebebc35769fd5b643d6a agents/lead_agent.md
5fe036308aa5ec9bf0cdaa8b1b341e5bcf09ff109fa7c27035f74f4492b91c71 agents/lead_agent.md
b189fb554d85d02346399cba00f1e8f42b450458cf395332a000d36224de333c agents/lead_agent.md
dc22e5fd2c360b3606aa3308016de8f91d6d354c7a71900782a30c401614e4b7 agents/lead_agent.md
f1fa78e010cfc6eaf4196b86d63b9edc9a7f7e66f098aa85f0c2da93017d9470 agents/lead_agent.md
f7cadcf5ba3d617929c58812dde09c0167de2588fb0bf580223bbe820b7cc1c3 agents/lead_agent.md
fc9d6d84c42e3b3ae46142e8a5c30de1568347774c5fdab98bccb4615a94e277 agents/lead_agent.md
0db51c778520fd0cf0293d717125387c6800fe68d40549e061ade6c6660019bf agents/planning_agent.md
1f56fabf740be46bb58b59f5d2c4fb7fa547e30019746c3f52f75a35ed00786b agents/planning_agent.md
7e96ed9af980a9b4180a3229d0e8efe0da956ff0e42d2c696af911838886761c agents/planning_agent.md
882d3493a2491da533b7d15e63fa624ac1b98c9db2805853bced6200d552a793 agents/planning_agent.md
a005953c84b2c69bfe57124f09c6c719778b038c0b49a07e235f6739ac2a2735 agents/planning_agent.md
cddf68c3b35cdfdb67df8c09a39fa16011b2a996a60d0ebacb4b0eb6fb10b472 agents/planning_agent.md
e37b380627df8025f457368d4bad6853cedc71c45af9892ae068ce8df162635e agents/planning_agent.md
326f9efd284d3803cc97e2775f789196173f40bb1b70421e2bd3fdeb436b8345 agents/research_agent.md
5de8abd44a061d39a87cffe203db11f492ce3409d7f8fe75e37196cf6511a772 agents/research_agent.md
7d950ef08bcd38e02e2013633f8d8c1540381ec2e90798814756f2b6dd699aac agents/research_agent.md
8e4a6af531c76accfb0357ca2814b7383f9a34ad9353aba1e92985324cb61039 agents/research_agent.md
9f89a724d7c529d20c4ba6316fdd0d02bab777289a84013ec60db6f88ee73a80 agents/research_agent.md
b8a30faa2d3ec6cf8a259502e81f2f243ad11eda0e6a92b2c6d2f8956f92b5e8 agents/research_agent.md
cf63ec1f1f1a78ddc78d3dd4afd8c0a7e96210f0bf05eb7a694f7734932a71f3 agents/research_agent.md
e96105549f8b2ae6b52a332815824854de889b5ac45fd14b678893cfe332f5fd agents/research_agent.md
f4542fb220f386ed22019c6e879e696d08d07598933f6ea01b4184f4ffb811ef agents/research_agent.md
4282ba9d287b8948be8e7e81bb242295b7a1d1631103a9ea2e1a826118d2afe9 agents/review_agent.md
4fb85482d613bfcf43fc1a56e06b67bfe9b068c5c512b135d880ce9210d554b5 agents/review_agent.md
6d8507995aadc909e75b2bc5135b74c2b512b31621a5de3f93bce30332210975 agents/review_agent.md
70ccce8bdcd285b41c3b96c31ebcf9b38b151404b16728fcfca70b91eadfc123 agents/review_agent.md
74ad03495909d68b55ab8a64b0acfd5b26cb14fb9ab36118c774f504a51103f4 agents/review_agent.md
80724c03623b5f64473067046dc5f2d50a92d5fa70ff15377ea1a3a17610aace agents/review_agent.md
ece44ebf6430ada43c20741fb568dc94439438e98e2af05bce95727c3d6eb85a agents/review_agent.md
01f896f47a47bff251cbe1be5219ab82fb0ee14258eb1508497c0344ff1df1f0 agents/system_agent.md manual-step-7
02e6c4ef0ebebd747c14646029116c9b871a92d6fdc4dbb21e727b09576b16c1 agents/system_agent.md
0df2738c3c7af7807f45465a86cc29d41bc9af99cc907a8f4a81173bc8ed4cdf agents/system_agent.md
1254aaa55fe5c9f411e4ef4bb754ac628317fd76394b259f2d2bb21ff0899c57 agents/system_agent.md
221b99e8602025dc89cce228a069a15631954529bcb33fe28aef7dff1ad4d9eb agents/system_agent.md
23663324b0e4151d33487d5bd7be1c69c119aed8ffd4b12e59cd8e0bbee203f1 agents/system_agent.md manual-step-7
3b7ced1f6eb41210dd3efbc792ae2baf9818d60d4132d391f4661f0b3ffe0224 agents/system_agent.md
40ed1cc9a6e3db383a4c1f56a2f891917c418395a4e713c82f8d2bae4661aee9 agents/system_agent.md
4a494a185f2b7e4352bc5c856d67fae92bc463dd533e946eb691fb6c557de74a agents/system_agent.md
661d45abe637d1a0daa188d3a55baa27e1a1203782a7f1889d720dbc402a6a4e agents/system_agent.md
695c9b0469bb5698d9d0ca0702ecd400cd3d31b0a3ab37b6df47efe06d319e65 agents/system_agent.md manual-step-7
ce6bb510a8057797669034b4eb62ac268eba029f9e052d9de857f529292972d0 agents/system_agent.md
d38f19d12c598ce0826e847b66974f7832de25c69c096ce26030b2a4b145c054 agents/system_agent.md manual-step-7
e7e4c5925d8422dea28c9db8f86c9f33c4ba3b59ef26c5a26b58fe9f9a52e652 agents/system_agent.md manual-step-7
f023828a8604ebe39d25610055f0391d26d13cb58676097dc9ba57d05d5e3425 agents/system_agent.md
f73c97a5584300901c0249b94643a4324c2b425b3cf54592f9cf4727f931434e agents/system_agent.md manual-step-7
040d98ccc2693ef22a963425f6be1583d571535ee8c99124f4fa5c351fe7f5fa agents/writing_agent.md
12f414952a326f10f3bec28ec42004e6705ec496227e460155da6a2394434b60 agents/writing_agent.md manual-step-8
20243874d72c455dee56f6632d34a6f7b9095ee94aed59c2df0e4aa7b6599f57 agents/writing_agent.md
240338cf2db18e93a03728e4a3a225b2631c6f21a86bdea829df4b8b51ae0e00 agents/writing_agent.md
2dc18ea41d778c7898489050e00a4a9935bdb24e4dc7e8833a673166df4f4048 agents/writing_agent.md
30eadb88a3f48706bc30a0d1a0de2e28fc51e72ed888a9e4481722125f494162 agents/writing_agent.md manual-step-8
32789cf9bfd2349c1511ac52ef310d289bd95902a63c4f36ce380ff7e2c0c4f1 agents/writing_agent.md
345ecdaf85c0168c226b7034fe024f0abadfaccc173789aaa5cb9f64a1a6cf2e agents/writing_agent.md
367039ae601f4d54292c8ea09f3198e815ac0a8dcfa1e7d90968fd3d3310b0da agents/writing_agent.md
6e8e924218c58878eae13749aaf3cea33edb038ff3b75377d11f1562ec767bf6 agents/writing_agent.md
9207e823210bef61543c00e1c77d1f9de01dd522896fd9c6c08d8a89ba7ba011 agents/writing_agent.md
cc6bf6790a38de7f5d509288fd5da267e43562819745986be4b7696c9a9662e3 agents/writing_agent.md manual-step-8
e78a6fc75aa5a9363218a07e9171824aec4801f03debad8ded2909c6faf21d5d agents/writing_agent.md
f971a13df5336a852a9b6379d7a9a857d3ae896c6d3b9ec5323e918a9364b182 agents/writing_agent.md
63af0bdf07c68d747f720809139a0c728e589edec5c04c6202e0dfa6b01cb782 skills/code-review/SKILL.md
75636f76c66dcef91ad086fedd08f948bac8c3a4100dbde3f22ab11ce50799ea skills/code-review/SKILL.md
2c1a732c6af455523ed7dbec01f151ea6de9af2f2fbbb53523960a0633c65441 skills/commit-message/SKILL.md
b96ef14114217e2b778477b3f5871cc5922a122dc3c5f23755fa697538055643 skills/commit-message/SKILL.md
024622c7589dc3e2071feaf8efae8d873921ece2a45c49d9a698f3d405ffdafa skills/create-skill/SKILL.md
0ed209cbf5440536113449c1de8f31053b593f2a786f89db0ffe2eb24c8d60f5 skills/create-skill/SKILL.md
18c3b87203b449971445a506496364725269f930a5a27eb07a949d594c982a1e skills/create-skill/SKILL.md
599d74e0e3249fe0deba36e208c01685eae9562a528063b1f37d8d70f3a53adb skills/create-skill/SKILL.md
df181561dbb6b3ee4908b79274738a739c7e3ea4420b1a097fed2e55fe1af6ad skills/create-skill/SKILL.md
dc5557f94ef032854c9e4ace7bc12735f1b984b893baec0eaa75c44b09950ede skills/create-skill/scripts/check_exists.sh
ff0b630771a22459f433ec515b9d1a2b32722d882918ddc54e7bd845e555b1e8 skills/create-skill/scripts/check_exists.sh
2ff6760bce4f7b32d307d8e44c26e93a9241462b6b31ade30af83844eb5b6654 skills/create-skill/scripts/list_skills.sh
9debc3abdca6aa760f7f161b4bbe2ec4c3b2c2ee409165d167e12c1e7a0202e2 skills/create-skill/scripts/list_skills.sh
e4ea08f8baee4e3e46326bdbf0e73ca91f627570e2b680623283be7cad69c416 skills/create-skill/scripts/list_skills.sh
6cfaaa9acaee277350d306412e928a2e7e8ae6111ea54c37f871f6926350c439 skills/create-skill/scripts/scaffold.sh
e6cf9fd1ae09206051892e5ca660ccea36210eb2d619959b8216d455692e791a skills/create-skill/scripts/scaffold.sh
f853200c77f728a77b8baac66eec54f2ed1c0ecbf46ad5a7be77544ec620a5b1 skills/create-skill/scripts/scaffold.sh
2656ecd178f29bc664e8e09bab32caf1828d287a4b71ad61e49afe061f6d2329 skills/create-skill/scripts/write_skill_md.sh
34f8a93fd1ee61c439ad09a06e1e625905fd07c0ea76b5ea5a0fef5a41879c04 skills/create-skill/scripts/write_skill_md.sh
4ca7437082472871db4b8126b55180f040df899e29371dcc0d58d2f2e28d0af6 skills/create-skill/scripts/write_skill_md.sh
e98066be56b086e9bc0dbd78c311f3b43558a23d1a43848e33ac47d633d1e972 skills/create-skill/scripts/write_skill_md.sh
aac08497db1cea3095a231346a591ce34b52a0e8a64213103352b89eed2c418f skills/explain-code/SKILL.md
f43411943bb68afdbc0b71565c226fe08052dd800e729a806727f2b3690c58d2 skills/explain-code/SKILL.md
";

fn listed(hash: &str, path: &str, edit: Option<ManualEdit>) -> bool {
    shipped_lines().any(|line| line.hash == hash && line.path == path && line.edit == edit)
}

/// V-3: a bundled file whose bytes are not in the list fails here, printing
/// the line to append — so a commit that edits a shipped file cannot land
/// without its new version listed, and an unedited copy of it is recognised
/// by every later build. Each bundled template also parses.
#[test]
fn every_bundled_file_is_a_shipped_version() {
    let mut missing = Vec::new();
    for file in BUNDLED_AGENTS.iter().chain(BUNDLED_SKILLS) {
        let hash = sha256_hex(file.contents.as_bytes());
        if !listed(&hash, file.path, None) {
            missing.push(format!("{hash} {}", file.path));
        }
    }
    for file in BUNDLED_AGENTS {
        parse_agent_markdown(file.contents)
            .unwrap_or_else(|e| panic!("the bundled {} does not parse: {e}", file.path));
    }
    for file in BUNDLED_SKILLS
        .iter()
        .filter(|f| f.path.ends_with("/SKILL.md"))
    {
        crate::middleware::skill::parse_skill_markdown(file.contents)
            .unwrap_or_else(|e| panic!("the bundled {} does not parse: {e}", file.path));
    }
    assert!(
        missing.is_empty(),
        "append to crates/openalpaca_core/src/bundled/shipped_versions.txt, \
         keeping it sorted by path then hash:\n{}",
        missing.join("\n")
    );
    assert_eq!(bundled_templates().len(), BUNDLED_AGENTS.len());
    assert_eq!(
        bundled_skill_entries(Path::new("/pack")).len(),
        bundled_skill_keys().count()
    );
}

/// V-3: the list is append-only. Every line of the floor is still there,
/// and every line of the list reads as one.
#[test]
fn the_shipped_versions_list_never_loses_a_line() {
    let floor: Vec<&str> = FLOOR.lines().collect();
    assert_eq!(floor.len(), 109, "the initial list");
    let list: Vec<&str> = SHIPPED_VERSIONS.lines().collect();
    for line in &floor {
        assert!(list.contains(line), "a line left the list: {line}");
    }
    assert_eq!(
        shipped_lines().count(),
        list.len(),
        "every line reads as `<sha256 hex> <path>[ manual-step-7|8]`"
    );
}

/// Every fixture file, by its path under `config/`.
fn fixtures() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bundled/fixtures");
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|file| {
            let relative = file.strip_prefix(&root).unwrap();
            // `<commit>/<path under config/>`
            let under_config: PathBuf = relative.components().skip(1).collect();
            (file, under_config.to_string_lossy().replace('\\', "/"))
        })
        .collect()
}

/// §9: each fixture is a version the list holds at its path — proof the
/// list was generated from the right versions, not only that the rule
/// works on bytes typed into a test — and each fixture the manual's step
/// applies to, with the step done as written, is the list's tagged line.
#[test]
fn every_fixture_is_a_shipped_version() {
    let files = fixtures();
    assert_eq!(files.len(), 13, "{files:?}");
    for (file, path) in &files {
        let bytes = std::fs::read(file).unwrap();
        assert!(
            listed(&sha256_hex(&bytes), path, None),
            "{} is not the list's version of {path}",
            file.display()
        );
    }

    for text in [
        test_support::SYSTEM_AGENT_9ED847FE,
        test_support::SYSTEM_AGENT_2BC29A80,
    ] {
        let done = step_7(text);
        assert!(listed(
            &sha256_hex(done.as_bytes()),
            "agents/system_agent.md",
            Some(ManualEdit::Step7)
        ));
    }
    let done = step_8(test_support::WRITING_AGENT_2BC29A80);
    assert!(listed(
        &sha256_hex(done.as_bytes()),
        "agents/writing_agent.md",
        Some(ManualEdit::Step8)
    ));
    // The step done on the last seeded copy is the fix the repository made.
    let fixed = step_7(test_support::SYSTEM_AGENT_EB98B820);
    assert_eq!(
        classify_agent_file(fixed.as_bytes()).map(|c| c.edit),
        Some(None),
        "eb98b820's system_agent.md with step 7 done is b79adc26's"
    );
}

/// V-2b: both sides of TEMPLATE-TIMEOUT are shipped versions — earlier
/// ones both since SA-1 (C6) gave `lead_agent.md` its new description and
/// persona; `e9f15916`'s side was the current one until then.
#[test]
fn both_sides_of_template_timeout_classify_as_shipped() {
    assert_eq!(
        classify_agent_file(test_support::LEAD_AGENT_9B1EA1D3.as_bytes()),
        Some(ShippedCopy {
            version: ShippedVersion::Earlier,
            edit: None
        })
    );
    assert_eq!(
        classify_agent_file(test_support::LEAD_AGENT_E9F15916.as_bytes()),
        Some(ShippedCopy {
            version: ShippedVersion::Earlier,
            edit: None
        })
    );
    assert_eq!(classify_agent_file(b"---\nid: mine\n---\n"), None);
}

/// V-2b: TEMPLATE-TEMPERATURE took the `temperature:` line out of the nine
/// shipped templates, so an unedited copy of the version before it — what
/// an earlier build seeded, or a copy of the repository's `config/` — is an
/// earlier shipped version, set aside like any other, and the bundled
/// template it differs from is the current one.
#[test]
fn the_side_before_template_temperature_classifies_as_an_earlier_shipped_version() {
    let before = test_support::LEAD_AGENT_4038C409;
    assert!(
        before.contains("\ntemperature: 0.3\n"),
        "the fixture is the side that set it"
    );
    assert_eq!(
        classify_agent_file(before.as_bytes()),
        Some(ShippedCopy {
            version: ShippedVersion::Earlier,
            edit: None
        })
    );
    let bundled = BUNDLED_AGENTS
        .iter()
        .find(|file| file.path == "agents/lead_agent.md")
        .expect("lead_agent.md is bundled");
    assert_eq!(
        classify_agent_file(bundled.contents.as_bytes()),
        Some(ShippedCopy {
            version: ShippedVersion::Current,
            edit: None
        })
    );
}

/// **T4.** The main loop's tool calls are attributed to `orchestrator`, so
/// that a confirmation card can name who is asking instead of reading
/// "unknown is blocked on this". That only works while the name belongs to
/// nobody else: a shipped template with the same id would make the
/// capability log ambiguous and would make the GUI call a real agent by
/// the assistant's name. The templates are embedded here, so this is where
/// the collision would be caught.
#[test]
fn no_shipped_agent_template_claims_the_main_loop_s_id() {
    let mut ids = Vec::new();
    for file in BUNDLED_AGENTS {
        let id = file
            .contents
            .lines()
            .find_map(|line| line.trim().strip_prefix("id:"))
            .map(|value| value.trim().trim_matches('"').to_string())
            .unwrap_or_else(|| panic!("{} has no `id:` in its frontmatter", file.path));
        ids.push(id);
    }

    assert_eq!(ids.len(), 9, "the nine shipped templates: {ids:?}");
    assert!(
        !ids.iter().any(|id| id == MAIN_LOOP_AGENT_ID),
        "`{MAIN_LOOP_AGENT_ID}` is the main loop's own id and must not \
         also be a template's: {ids:?}"
    );
    // And it is a name, not the absence of one.
    assert_ne!(MAIN_LOOP_AGENT_ID, "unknown");
    assert!(!MAIN_LOOP_AGENT_ID.is_empty());
}

// ── The skill pack (§2.6 K-2) ────────────────────────────────────────────

/// Every file and folder under `root`, each with its mode, relative and
/// `/`-separated.
#[cfg(unix)]
fn modes_under(root: &Path) -> Vec<(String, u32)> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u32)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, meta.permissions().mode() & 0o777));
            if meta.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// K-2: the pack is the bundle, whole, at each start — a file a person put
/// in it, or edited, does not outlive the next write — and its modes say
/// "not for editing": files 0444, scripts 0555, folders 0755.
#[cfg(unix)]
#[test]
fn the_pack_is_rewritten_whole_at_each_start_and_read_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let _guard = crate::test_util::HomeStoreGuard::set(&dir.path().join("home"));
    let pack = dir.path().join("state/cache/bundled-skills");

    write_skill_pack(&pack).unwrap();
    // A hand edit and a stray file inside the pack.
    let skill_md = pack.join("code-review/SKILL.md");
    std::fs::set_permissions(&skill_md, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&skill_md, "edited").unwrap();
    std::fs::write(pack.join("code-review/notes.txt"), "mine").unwrap();
    write_skill_pack(&pack).unwrap();

    let mut expected: Vec<(String, u32)> = Vec::new();
    for file in BUNDLED_SKILLS {
        let rel = file.path.strip_prefix("skills/").unwrap();
        let mode = if rel.ends_with(".sh") { 0o555 } else { 0o444 };
        expected.push((rel.to_string(), mode));
        assert_eq!(
            std::fs::read_to_string(pack.join(rel)).unwrap(),
            file.contents,
            "{rel} is the bundle's"
        );
    }
    for folder in [
        "code-review",
        "commit-message",
        "create-skill",
        "create-skill/scripts",
        "explain-code",
    ] {
        expected.push((folder.to_string(), 0o755));
    }
    expected.sort();
    assert_eq!(modes_under(&pack), expected);
    assert_eq!(
        std::fs::read_dir(pack.parent().unwrap()).unwrap().count(),
        1,
        "nothing beside the pack is left"
    );
}

/// K-2: a `bundled-skills.new/` a crash left — even a read-only one — is
/// removed before the next write, and never mistaken for the pack.
#[cfg(unix)]
#[test]
fn a_leftover_new_pack_directory_is_removed_at_start() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let _guard = crate::test_util::HomeStoreGuard::set(&dir.path().join("home"));
    let cache = dir.path().join("state/cache");
    let leftover = cache.join("bundled-skills.new/half-written");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("SKILL.md"), "half").unwrap();
    std::fs::set_permissions(
        leftover.join("SKILL.md"),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();

    write_skill_pack(&cache.join("bundled-skills")).unwrap();

    let mut names: Vec<String> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["bundled-skills"]);
    assert!(!cache.join("bundled-skills/half-written").exists());
}

/// V-5 against a bundle that gained a file: an unedited copy of an earlier
/// version that lacks it is still an unedited copy, and so is a copy of the
/// current version missing one of its files.
#[test]
fn a_skill_folder_missing_a_shipped_file_is_still_an_unedited_copy() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = crate::test_util::HomeStoreGuard::set(&dir.path().join("home"));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/bundled/fixtures/82022776/skills/create-skill");
    let mut gained: Vec<BundledFile> = BUNDLED_SKILLS
        .iter()
        .map(|f| BundledFile {
            path: f.path,
            contents: f.contents,
        })
        .collect();
    gained.push(BundledFile {
        path: "skills/create-skill/scripts/fifth.sh",
        contents: "#!/bin/sh\n",
    });
    assert_eq!(
        classify_skill_dir_against(&gained, "create-skill", &fixture),
        Some(SkillFolder::UneditedCopy(ShippedVersion::Earlier))
    );

    let copy = dir.path().join("create-skill");
    for file in BUNDLED_SKILLS
        .iter()
        .filter(|f| f.path.starts_with("skills/create-skill/"))
    {
        let target = copy.join(file.path.strip_prefix("skills/create-skill/").unwrap());
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, file.contents).unwrap();
    }
    std::fs::remove_file(copy.join("scripts/check_exists.sh")).unwrap();
    assert_eq!(
        classify_skill_dir("create-skill", &copy),
        Some(SkillFolder::UneditedCopy(ShippedVersion::Current))
    );
    assert_eq!(classify_skill_dir("my-skill", &copy), None, "V-6");
}
