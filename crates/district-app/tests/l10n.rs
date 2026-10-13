//! French from a compiled catalog, the way the app reads it: a template's
//! `translatable="yes" context="<key>"` string, built by GtkBuilder and as a
//! composite template, and the language each side of the app resolves, which
//! the app compares at start-up (`l10n::report_disagreement`).
//!
//! The catalog is a stand-in with one French string, under a key the real
//! templates use: the app's own has no French until it is reviewed. It is
//! compiled with `msgfmt` into a temporary `<dir>/fr/LC_MESSAGES/district-ai.mo`,
//! then this test runs itself once per case with the case's environment,
//! because the locale is read from the environment by GTK's own
//! `setlocale(LC_ALL, "")` and this workspace forbids the unsafe
//! `std::env::set_var`. glibc ignores `LANGUAGE` under the C locales, so the
//! French cases need `fr_CA.UTF-8` and `en_US.UTF-8` generated (CI runs
//! `localedef`). Needs a display, like the smoke test:
//!
//! ```text
//! GDK_BACKEND=x11 GTK_A11Y=none xvfb-run -a dbus-run-session -- \
//!   cargo test -p district-app --features gtk-tests --test l10n
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use district_app::l10n::{self, Locale};
use gtk4 as gtk;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, glib};

/// The key and its English, as `data/ui/support-page.ui` has them.
const KEY: &str = "support_page.support";
const ENGLISH: &str = "Support";
const FRENCH: &str = "Assistance";

/// A widget built from a template, as every page of the app is.
mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(string = r#"
<interface>
  <template class="L10nTestBox" parent="GtkBox">
    <child>
      <object class="GtkLabel" id="label">
        <property name="label" translatable="yes" context="support_page.support">Support</property>
      </object>
    </child>
  </template>
</interface>"#)]
    pub struct TestBox {
        #[template_child]
        pub label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TestBox {
        const NAME: &'static str = "L10nTestBox";
        type Type = super::TestBox;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TestBox {}
    impl WidgetImpl for TestBox {}
    impl BoxImpl for TestBox {}
}

glib::wrapper! {
    /// See [`imp::TestBox`].
    pub struct TestBox(ObjectSubclass<imp::TestBox>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

const BUILDER: &str = r#"
<interface>
  <object class="GtkLabel" id="label">
    <property name="label" translatable="yes" context="support_page.support">Support</property>
  </object>
</interface>"#;

/// In the child: bind the catalogs, initialise GTK, and print what each
/// lookup gives and the language each side resolved.
fn child(dir: &Path) {
    l10n::init_at(dir);
    gtk::init().expect("a display");
    let lookup = l10n::pgettext(KEY, ENGLISH);
    let builder = gtk::Builder::from_string(BUILDER)
        .object::<gtk::Label>("label")
        .expect("the label")
        .label();
    let template = glib::Object::new::<TestBox>().imp().label.label();
    let core = Locale::from_tags(&glib::language_names());
    let gettext = l10n::gettext_locale();
    let agree = l10n::disagreement(core, gettext).is_none();
    println!(
        "lookup={lookup}|builder={builder}|template={template}|core={}|gettext={}|agree={agree}",
        core.tag(),
        gettext.tag()
    );
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

/// A catalog with one French string, compiled where the app would look.
fn compile_catalog() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("district-l10n-test-{}", std::process::id()));
    let messages = dir.join("fr/LC_MESSAGES");
    std::fs::create_dir_all(&messages).expect("a temporary directory");
    let po = dir.join("fr.po");
    std::fs::write(
        &po,
        format!(
            "msgid \"\"\nmsgstr \"\"\n\"Language: fr_CA\\n\"\n\
             \"Content-Type: text/plain; charset=UTF-8\\n\"\n\n\
             msgctxt \"{KEY}\"\nmsgid \"{ENGLISH}\"\nmsgstr \"{FRENCH}\"\n"
        ),
    )
    .expect("the catalog is written");
    let status = Command::new("msgfmt")
        .arg("--check")
        .arg("-o")
        .arg(messages.join(format!("{}.mo", l10n::DOMAIN)))
        .arg(&po)
        .status()
        .expect("msgfmt (the gettext package) is installed");
    assert!(status.success(), "msgfmt refused {}", po.display());
    dir
}

fn line(text: &str, core: &str, gettext: &str) -> String {
    format!(
        "lookup={text}|builder={text}|template={text}|core={core}|gettext={gettext}|agree={}",
        core == gettext
    )
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == "child") {
        child(Path::new(&args.next().expect("the catalog directory")));
        return;
    }
    let dir = compile_catalog();
    let french = line(FRENCH, "fr-CA", "fr-CA");
    let english = line(ENGLISH, "en", "en");
    let cases: [(&[(&str, &str)], Vec<String>); 5] = [
        // The usual French desktop: both sides French.
        (&[("LANG", "fr_CA.UTF-8")], vec![french.clone()]),
        // LANGUAGE over another real locale: both sides French.
        (
            &[("LANG", "en_US.UTF-8"), ("LANGUAGE", "fr_CA:fr")],
            vec![french.clone()],
        ),
        // LANGUAGE under C.UTF-8: the core says French, and gettext follows
        // it or, as recent glibc does under the C locales, ignores it, so the
        // templates stay English, which start-up reports. Either way the
        // templates and the language gettext names agree.
        (
            &[("LANG", "C.UTF-8"), ("LANGUAGE", "fr")],
            vec![line(ENGLISH, "fr-CA", "en"), french],
        ),
        // English asked for.
        (&[("LANG", "en_US.UTF-8")], vec![english.clone()]),
        // No language asked for.
        (&[("LANG", "C.UTF-8")], vec![english]),
    ];
    let mut failed = Vec::new();
    for (env, wanted) in cases {
        let got = run(&dir, env);
        if !wanted.contains(&got) {
            failed.push(format!("{env:?}: got {got}, want one of {wanted:?}"));
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(failed.is_empty(), "{failed:#?}");
    println!("l10n test passed");
}
