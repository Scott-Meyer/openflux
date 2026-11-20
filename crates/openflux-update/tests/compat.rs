mod common;

use openflux_update::compat::{Compat, CompatError};
use openflux_update::version::{Component, InterfaceVersion};

const REPO: &str = include_str!("../../../compat.toml");
const WORKSPACE_MANIFEST: &str = include_str!("../../../Cargo.toml");
const BRIDGE_MANIFEST: &str = include_str!("../../../keyboard/bridge/Cargo.toml");
const DISPLAY_MANIFEST: &str = include_str!("../../../keyboard/display/Cargo.toml");
const FIRMWARE_CMAKE: &str = include_str!("../../../keyboard/firmware/CMakeLists.txt");

/// The version a component's own build files declare.
fn manifest_version(c: Component) -> semver::Version {
    let cargo = |text: &str, table: &str| -> String {
        let doc: toml::Table = text.parse().unwrap();
        let mut t = &doc;
        for key in table.split('.') {
            t = t[key].as_table().unwrap();
        }
        t["version"].as_str().unwrap().to_owned()
    };
    let text = match c {
        Component::Host => cargo(WORKSPACE_MANIFEST, "workspace.package"),
        Component::Bridge => cargo(BRIDGE_MANIFEST, "package"),
        Component::Display => cargo(DISPLAY_MANIFEST, "package"),
        Component::Firmware => FIRMWARE_CMAKE
            .lines()
            .find_map(|l| l.trim().strip_prefix("set(OPENFLUX_FIRMWARE_VERSION \""))
            .and_then(|rest| rest.split('"').next())
            .expect("CMakeLists.txt sets OPENFLUX_FIRMWARE_VERSION")
            .to_owned(),
    };
    text.parse().unwrap()
}

#[test]
fn repository_registry_is_consistent() {
    let compat = Compat::parse(REPO).unwrap();
    for c in Component::ALL {
        assert_eq!(compat.components[&c].version, manifest_version(c), "{c}");
    }
    for name in compat.interfaces.keys() {
        assert!(compat.provider(name).is_some(), "{name}");
    }
    let host = compat.host_profile();
    assert_eq!(host.transport(Component::Firmware), ["firmware-usb"]);
    assert_eq!(host.transport(Component::Bridge), ["bridge-usb"]);
    assert_eq!(host.transport(Component::Display), ["bridge-usb"]);
    assert!(host.transport(Component::Host).is_empty());
    let bridge = compat.interface_set(Component::Bridge);
    let uart = compat.interfaces["uart"].version;
    assert_eq!(bridge.built_with["uart"], uart);
    assert!(bridge.required("uart").iter().any(|r| r.accepts(uart)));
}

fn edit(from: &str, to: &str) -> String {
    let base = common::REGISTRY;
    assert!(base.contains(from), "fixture lacks {from:?}");
    base.replacen(from, to, 1)
}

#[test]
fn invalid_registries_are_explained() {
    let cases: Vec<(&str, String, &str)> = vec![
        ("requirement not met by the provider", edit("requires = { uart = \">=1.0, <2.0\" }", "requires = { uart = \">=1.3, <2.0\" }"), "bridge requires uart >=1.3, <2.0 but firmware provides 1.0"),
        ("two providers", edit("provides = [\"config\"]", "provides = [\"config\", \"uart\"]"), "interface uart has several providers"),
        ("unregistered interface", edit("provides = [\"config\"]", "provides = [\"config\", \"video\"]"), "display provides unregistered interface video"),
        ("interface nobody provides", edit("[interfaces.config]", "[interfaces.spare]\nversion = \"1.0\"\nkind = \"data\"\nsummary = \"x\"\n\n[interfaces.config]"), "interface spare has no provider"),
        ("unknown component name", edit("[components.display]", "[components.display-x]"), "not valid TOML for the registry"),
        ("range spanning majors", edit("requires = { uart = \">=1.0, <2.0\" }", "requires = { uart = \">=1.0, <3.0\" }"), "must stay within one major"),
        ("keyboard component without transport", edit("update-transport = [\"firmware-usb\"]", ""), "firmware needs an update-transport"),
        ("transport the host does not speak", edit("update-transport = [\"firmware-usb\"]", "update-transport = [\"uart\"]"), "firmware update-transport uart is not required by the host"),
        ("also-provides of the current major", edit("provides = [\"bridge-usb\", \"display-ipc\"]", "provides = [\"bridge-usb\", \"display-ipc\"]\nalso-provides = { bridge-usb = [\"1.0\"] }"), "also-provides is for older majors"),
        ("unknown field", edit("kind = \"data\"\nsummary = \"keymap\"", "kind = \"data\"\nsummary = \"keymap\"\ncolour = \"blue\""), "unknown field"),
        ("bad interface version", edit("version = \"1.0\"\nkind = \"protocol\"\nsummary = \"bridge to display\"", "version = \"1\"\nkind = \"protocol\"\nsummary = \"bridge to display\""), "expected MAJOR.MINOR"),
    ];
    let mut failures = Vec::new();
    for (name, text, want) in cases {
        match Compat::parse(&text) {
            Ok(_) => failures.push(format!("{name}: accepted")),
            Err(e @ (CompatError::Syntax(_) | CompatError::Invalid(_))) => {
                if !e.to_string().contains(want) {
                    failures.push(format!("{name}: want {want:?} in {e}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn transitional_majors_are_provided_alongside_the_current_one() {
    let text = edit(
        "[interfaces.bridge-usb]\nversion = \"1.0\"",
        "[interfaces.bridge-usb]\nversion = \"2.0\"",
    )
    .replace(
        "bridge-usb = \">=1.0, <2.0\"",
        "bridge-usb = [\">=1.4, <2.0\", \">=2.0, <3.0\"]",
    )
    .replace(
        "provides = [\"bridge-usb\", \"display-ipc\"]",
        "provides = [\"bridge-usb\", \"display-ipc\"]\nalso-provides = { bridge-usb = [\"1.4\"] }",
    );
    let compat = Compat::parse(&text).unwrap();
    let bridge = compat.interface_set(Component::Bridge);
    assert_eq!(
        bridge.provided("bridge-usb"),
        [InterfaceVersion::new(1, 4), InterfaceVersion::new(2, 0)]
    );
    assert_eq!(
        compat
            .interface_set(Component::Host)
            .required("bridge-usb")
            .len(),
        2
    );
}
