//! SPIKE (v3.0 W0, do not merge): does a compiled `district-ai.mo` reach the
//! three places the app reads strings from (a Rust lookup, a GtkBuilder
//! `translatable="yes" context="..."` property, and a composite template's),
//! and under which locale settings?
//!
//! It compiles `po/fr.po` with `msgfmt` into a temporary
//! `<dir>/fr/LC_MESSAGES/district-ai.mo`, then runs itself once per case with
//! the case's environment, because the locale is read from the environment by
//! GTK's own `setlocale(LC_ALL, "")` and this workspace forbids the unsafe
//! `std::env::set_var`. Needs a display, like the smoke test:
//!
//! ```text
//! GDK_BACKEND=x11 GTK_A11Y=none xvfb-run -a dbus-run-session -- \
//!   cargo test -p district-app --features gtk-tests --test l10n
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use gtk4 as gtk;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, glib};

/// A widget built from a template, as every page of the app is.
mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(string = r#"
<interface>
  <template class="SpikeL10nBox" parent="GtkBox">
    <child>
      <object class="GtkLabel" id="label">
        <property name="label" translatable="yes" context="support-page">Support</property>
      </object>
    </child>
  </template>
</interface>"#)]
    pub struct SpikeBox {
        #[template_child]
        pub label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SpikeBox {
        const NAME: &'static str = "SpikeL10nBox";
        type Type = super::SpikeBox;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SpikeBox {}
    impl WidgetImpl for SpikeBox {}
    impl BoxImpl for SpikeBox {}
}

glib::wrapper! {
    /// See [`imp::SpikeBox`].
    pub struct SpikeBox(ObjectSubclass<imp::SpikeBox>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

/// A case's environment, and the line it must print when that is asserted.
type Case<'a> = (&'a [(&'a str, &'a str)], Option<&'a String>);

const BUILDER: &str = r#"
<interface>
  <object class="GtkLabel" id="label">
    <property name="label" translatable="yes" context="support-page">Support</property>
  </object>
</interface>"#;

/// In the child: bind the catalogs, initialise GTK, and print what each of
/// the three lookups gives.
fn child(dir: &Path) {
    district_app::l10n::init_at(dir);
    gtk::init().expect("a display");
    let rust = district_app::l10n::pgettext("spike", "Settings");
    let builder = gtk::Builder::from_string(BUILDER)
        .object::<gtk::Label>("label")
        .expect("the label")
        .label();
    let template = glib::Object::new::<SpikeBox>().imp().label.label();
    let probe = district_app::l10n::probe_line();
    println!("rust={rust}|builder={builder}|template={template}|probe={probe}");
}

/// Runs this test as a child with `env` (and none of the locale variables it
/// does not name), and returns the line it printed.
fn run(dir: &Path, env: &[(&str, &str)]) -> String {
    let mut command = Command::new(std::env::current_exe().expect("this test"));
    command.arg("child").arg(dir);
    for name in ["LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES"] {
        command.env_remove(name);
    }
    command.envs(env.iter().copied());
    let output = command.output().expect("the child runs");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    assert!(
        output.status.success(),
        "{env:?}: {stdout} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{env:?} -> {stdout}");
    stdout
}

fn compile_catalog() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("district-l10n-spike-{}", std::process::id()));
    let messages = dir.join("fr/LC_MESSAGES");
    std::fs::create_dir_all(&messages).expect("a temporary directory");
    let po = Path::new(env!("CARGO_MANIFEST_DIR")).join("po/fr.po");
    let status = Command::new("msgfmt")
        .arg("--check")
        .arg("-o")
        .arg(messages.join("district-ai.mo"))
        .arg(&po)
        .status()
        .expect("msgfmt (the gettext package) is installed");
    assert!(status.success(), "msgfmt refused {}", po.display());
    dir
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == "child") {
        child(Path::new(&args.next().expect("the catalog directory")));
        return;
    }
    let dir = compile_catalog();
    let french =
        "rust=Paramètres|builder=Assistance|template=Assistance|probe=Assistance".to_owned();
    let english = "rust=Settings|builder=Support|template=Support|probe=Support".to_owned();

    // Each case and whether its result is asserted. glibc ignores LANGUAGE
    // in the C locale, and (measured on glibc 2.43; CI logs 2.39) in C.UTF-8 too,
    // so LANGUAGE alone needs a real locale beside it. A locale that is not
    // installed leaves setlocale at "C".
    let recorded: [Case; 5] = [
        // The usual French desktop. Needs fr_CA.UTF-8 generated.
        (&[("LANG", "fr_CA.UTF-8")], None),
        // LANGUAGE with any other real locale. Needs en_US.UTF-8 generated.
        (&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "fr_CA:fr")], None),
        // LANGUAGE with the C.UTF-8 locale every glibc now carries built in.
        (&[("LANG", "C.UTF-8"), ("LANGUAGE", "fr")], None),
        // LANGUAGE with the plain C locale: ignored, by design of gettext.
        (&[("LANG", "C"), ("LANGUAGE", "fr")], Some(&english)),
        // No French asked for.
        (&[("LANG", "C.UTF-8")], Some(&english)),
    ];
    let mut failed = Vec::new();
    let mut french_seen = false;
    for (env, want) in recorded {
        let got = run(&dir, env);
        french_seen |= got == french;
        if let Some(want) = want
            && got != *want
        {
            failed.push(format!("{env:?}: got {got}, want {want}"));
        }
    }
    if !french_seen {
        failed.push(format!(
            "no case gave {french}: is any non-C locale installed?"
        ));
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(failed.is_empty(), "{failed:#?}");
    println!("l10n spike test passed");
}
