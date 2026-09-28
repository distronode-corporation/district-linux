//! The store screenshots: the pictures Flathub and the software centres show,
//! drawn from the real window.
//!
//! Like the smoke test (`tests/smoke.rs`), this builds the whole window
//! against a scripted stand-in for the effect runner and answers what the app
//! asks for. Unlike it, the answers are written here, for an invented plumbing
//! business, so that the pictures read like a working day rather than like
//! test data: every business, person and email address in them is made up,
//! and every phone number is in the fictional 555-0100 to 555-0199 range.
//!
//! It draws five scenes in the light style: the overview, the inbox with a
//! thread open, the call log with a call's transcript, contacts, and a call
//! under way. Each is a window of 1000 by 700, the largest Flathub's quality
//! guidelines allow, drawn with the rounded corners and the shadow a
//! compositing desktop gives it (see [`Store::render`]), so each picture is a
//! little larger than the window. With `DISTRICT_STORE_SHOTS` set to a
//! directory it saves them there as PNGs, checking each is what Flathub asks
//! for; without it, it only draws them, which is how CI runs it, so the scenes
//! keep working as the window changes. The pictures under
//! `crates/district-app/data/screenshots/` come from it, and CONTRIBUTING.md
//! ("Store screenshots") says how to make them again.
//!
//! It needs what the smoke test needs, a display and a session bus, and is
//! built only with the `gtk-tests` feature:
//!
//! ```text
//! GSK_RENDERER=cairo GDK_BACKEND=x11 GTK_A11Y=none GTK_MEDIA=none \
//!   GSETTINGS_BACKEND=memory xvfb-run -a -s "-screen 0 1280x1024x24" \
//!   dbus-run-session -- cargo test -p district-app --features gtk-tests \
//!   --test store_screenshots
//! ```

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use district_app::{Effects, Parts, UiBridge, application};
use district_auth::AccessClaims;
use district_core::{
    CALL_TICK, CoreConfig, Effect, Event, MediaEvent, MediaUpdate, MicrophoneState, Participant,
    Ticket, format_phone_number,
};
use district_model::{CallSummary, ContactDetailResponse, ContactListResponse};
use gtk::{gdk, gio, glib, gsk};
use gtk4 as gtk;
use libadwaita as adw;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// The window's size in every scene: the largest Flathub asks for, so the
/// three panes of the inbox, the call log and contacts all fit.
const WIDTH: i32 = 1000;
const HEIGHT: i32 = 700;

/// The invented business, and its one workspace.
const WORKSPACE: &str = "ws-kestrel-row";
const BUSINESS: &str = "Kestrel Row Plumbing";

/// The scripted stand-in for the effect runner: every effect is kept for the
/// script to answer, or not.
#[derive(Default)]
struct Script {
    pending: RefCell<VecDeque<Effect>>,
}

impl Effects for Script {
    fn run(&self, effect: Effect) {
        self.pending.borrow_mut().push_back(effect);
    }

    fn settle(&self, effects: Vec<Effect>, done: tokio::sync::oneshot::Sender<()>) {
        self.pending.borrow_mut().extend(effects);
        done.send(()).ok();
    }

    fn finish(&self, _effects: Vec<Effect>, _limit: Duration) {}
}

/// The app under the script, and where the pictures go, if anywhere.
struct Store {
    app: adw::Application,
    script: Rc<Script>,
    events: async_channel::Sender<Event>,
    /// Held so the channel the app reads its commands from stays open.
    _bridge: UiBridge,
    shots: Option<PathBuf>,
}

/// The ticket an effect carries, for the ones the script answers.
fn ticket(effect: &Effect) -> Ticket {
    match effect {
        Effect::RestoreSession { ticket }
        | Effect::LoadWorkspaces { ticket }
        | Effect::LoadOverview { ticket, .. }
        | Effect::LoadSetupStatus { ticket, .. }
        | Effect::LoadUnreadCount { ticket, .. }
        | Effect::LoadConversations { ticket, .. }
        | Effect::LoadDraftKeys { ticket, .. }
        | Effect::LoadDraft { ticket, .. }
        | Effect::LoadTimeline { ticket, .. }
        | Effect::MarkRead { ticket, .. }
        | Effect::LoadCalls { ticket, .. }
        | Effect::LoadCall { ticket, .. }
        | Effect::LoadTranscript { ticket, .. }
        | Effect::LoadContacts { ticket, .. }
        | Effect::LoadContact { ticket, .. }
        | Effect::ReadRingSetting { ticket }
        | Effect::Dial { ticket, .. }
        | Effect::Wait { ticket, .. } => *ticket,
        other => panic!("no ticket the script answers in {other:?}"),
    }
}

/// Every widget under `root`, depth first.
fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut found = vec![root.clone()];
    let mut child = root.first_child();
    while let Some(widget) = child {
        found.extend(descendants(&widget));
        child = widget.next_sibling();
    }
    found
}

/// `node` without the clip a widget paintable puts around a widget's own
/// box, keeping every transform above it.
fn unclipped(node: gsk::RenderNode) -> gsk::RenderNode {
    if let Some(clip) = node.downcast_ref::<gsk::ClipNode>() {
        return clip.child();
    }
    if let Some(transform) = node.downcast_ref::<gsk::TransformNode>() {
        return gsk::TransformNode::new(unclipped(transform.child()), Some(&transform.transform()))
            .upcast();
    }
    node
}

impl Store {
    /// Runs the main loop until it has nothing left to do, a few times over,
    /// so a frame is laid out and drawn.
    fn pump(&self) {
        let context = glib::MainContext::default();
        for _ in 0..4 {
            while context.iteration(false) {}
            std::thread::sleep(Duration::from_millis(15));
        }
        while context.iteration(false) {}
    }

    /// Sends `event` as if the runner had, and lets the app draw it.
    fn answer(&self, event: Event) {
        self.events.try_send(event).expect("the app is reading");
        self.pump();
    }

    /// The first pending effect whose `Debug` starts with `name`, taken off
    /// the list.
    fn take(&self, name: &str) -> Effect {
        let mut pending = self.script.pending.borrow_mut();
        let index = pending
            .iter()
            .position(|effect| format!("{effect:?}").starts_with(name))
            .unwrap_or_else(|| panic!("no {name} among {pending:?}"));
        pending.remove(index).expect("the index is in range")
    }

    /// Whether an effect whose `Debug` starts with `name` is pending.
    fn pending(&self, name: &str) -> bool {
        self.script
            .pending
            .borrow()
            .iter()
            .any(|effect| format!("{effect:?}").starts_with(name))
    }

    /// Answers the pending effect named `name` with the event `answer` makes
    /// from its ticket.
    fn reply(&self, name: &str, answer: impl FnOnce(Ticket) -> Event) {
        let ticket = ticket(&self.take(name));
        self.answer(answer(ticket));
    }

    /// Forgets what the app asked for so far: the script answers none of it.
    fn forget(&self) {
        self.script.pending.borrow_mut().clear();
    }

    fn window(&self) -> gtk::Window {
        self.app.active_window().expect("the app has a window")
    }

    /// The mapped widgets with the template id or the widget name `name`.
    fn mapped(&self, name: &str) -> Vec<gtk::Widget> {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(|widget| {
                widget.is_mapped()
                    && (widget.buildable_id().as_deref() == Some(name)
                        || widget.widget_name() == name)
            })
            .collect()
    }

    /// The first mapped widget named `name`.
    fn one(&self, name: &str) -> gtk::Widget {
        self.mapped(name)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{name} is not on screen"))
    }

    fn click(&self, name: &str) {
        self.one(name)
            .downcast::<gtk::Button>()
            .expect("a button")
            .emit_clicked();
        self.pump();
    }

    /// Activates the `index`th mapped row named `name`.
    fn activate(&self, name: &str, index: usize) {
        let row = self
            .mapped(name)
            .into_iter()
            .nth(index)
            .unwrap_or_else(|| panic!("no row {index} named {name}"));
        row.activate();
        self.pump();
    }

    /// Whether a mapped label shows exactly `text`.
    fn shows(&self, text: &str) -> bool {
        descendants(self.window().upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .filter_map(|widget| widget.downcast::<gtk::Label>().ok())
            .any(|label| label.label() == text)
    }

    /// The window as a compositing desktop draws it: rounded, with its
    /// shadow, on nothing.
    ///
    /// Xvfb has no compositing manager, so GTK gives the window its
    /// `solid-csd` style, square and without a shadow, which is how it looks
    /// on a desktop without one. Flathub asks for the rounded corners and the
    /// shadow, so the window is given the `csd` style GTK gives it on a
    /// compositing desktop, and the shadow libadwaita's stylesheet then draws
    /// is kept: a widget paintable clips a widget to its own box, and the
    /// shadow lies outside it. Nothing here draws a shadow of its own.
    fn render(&self) -> gdk::Texture {
        let window = self.window();
        window.remove_css_class("solid-csd");
        window.add_css_class("csd");
        self.pump();
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(
            &snapshot,
            f64::from(window.width()),
            f64::from(window.height()),
        );
        let node = unclipped(snapshot.to_node().expect("the window drew something"));
        window
            .renderer()
            .expect("a realised window")
            .render_texture(node, None)
    }

    /// Draws the scene, and saves it as `name`.png when pictures were asked
    /// for.
    fn shot(&self, name: &str) {
        let texture = self.render();
        assert!(texture.width() > 0 && texture.height() > 0);
        let Some(dir) = &self.shots else {
            return;
        };
        // What Flathub asks of a picture, checked only for the pictures kept,
        // which are made on one machine rather than on every CI runner: the
        // window no larger than 1000 by 700, its shadow around it, and
        // nothing behind it.
        let window = self.window();
        assert_eq!((window.width(), window.height()), (WIDTH, HEIGHT));
        assert!(
            texture.width() > WIDTH && texture.height() > HEIGHT,
            "the picture has the window's shadow"
        );
        let mut downloader = gdk::TextureDownloader::new(&texture);
        downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
        let (pixels, _stride) = downloader.download_bytes();
        assert!(pixels[3] < u8::MAX, "the corner is transparent");
        texture
            .save_to_png(dir.join(format!("{name}.png")))
            .expect("the screenshot is written");
        println!("{name}.png: {}x{}", texture.width(), texture.height());
    }
}

/// `value` read as the service's answer would be.
fn from<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
}

/// Twenty to twelve this morning, the moment the pictures show: every time in
/// them is set back from it, so the lists read "today" whenever the pictures
/// are made, and a run just after midnight does not split the day.
fn morning() -> glib::DateTime {
    let now = glib::DateTime::now_local().expect("the time");
    glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 11, 40, 0.0)
        .expect("this morning")
}

/// The moment `minutes` before [`morning`], as the service writes one.
fn ago(minutes: i64) -> String {
    morning()
        .add_minutes(-i32::try_from(minutes).expect("a few weeks"))
        .and_then(|then| then.to_utc())
        .and_then(|then| then.format("%Y-%m-%dT%H:%M:00.000Z"))
        .expect("a time")
        .to_string()
}

/// A call in the log, the overview and the call's own screen. `name` is what
/// the service shows for the caller (their name, or their number), and
/// `seconds` how long it lasted, 0 for one nobody answered.
struct Call<'a> {
    id: &'a str,
    name: &'a str,
    number: &'a str,
    direction: &'a str,
    minutes_ago: i64,
    seconds: i64,
    summary: &'a str,
}

impl Call<'_> {
    fn summary(&self) -> CallSummary {
        let answered = self.seconds > 0;
        let duration = if self.seconds >= 60 {
            format!("{}m {}s", self.seconds / 60, self.seconds % 60)
        } else {
            format!("{}s", self.seconds)
        };
        let call_type = match (self.direction, answered) {
            (_, false) => "missed",
            ("outbound", true) => "outbound",
            _ => "inbound",
        };
        let listed = if self.summary.is_empty() {
            "No summary available."
        } else {
            self.summary
        };
        from(json!({
            "id": self.id,
            "type": call_type,
            "number": self.name,
            "status": if answered { "completed" } else { "no-answer" },
            "duration": duration,
            "time": "",
            "aiSummary": listed,
            "recordingUrl": null,
            "transcript": "",
            "hasTranscript": answered,
            "callerName": self.name,
            "from": self.number,
            "direction": self.direction,
            "durationRaw": self.seconds,
            "summary": self.summary,
            "createdAt": ago(self.minutes_ago),
            "followUp": null,
            "sentiment": null,
            "disposition": null,
            "analysis": null,
            "transferStatus": null,
            "transferReason": null,
            "phoneIntel": null
        }))
    }
}

/// The call log, newest first.
const CALLS: [Call<'static>; 8] = [
    Call {
        id: "call-rosa",
        name: "Rosa Delgado",
        number: "+13125550142",
        direction: "inbound",
        minutes_ago: 190,
        seconds: 142,
        summary: "Kitchen sink leaking under the cabinet. Booked a repair visit for \
                  Thursday between 9 and 11 am.",
    },
    Call {
        id: "call-weekend",
        name: "+13125550189",
        number: "+13125550189",
        direction: "inbound",
        minutes_ago: 230,
        seconds: 48,
        summary: "Asked whether we come out for emergencies on weekends. Took a number \
                  for a call back.",
    },
    Call {
        id: "call-owen",
        name: "Owen Barker",
        number: "+13125550164",
        direction: "outbound",
        minutes_ago: 1_210,
        seconds: 245,
        summary: "Talked through replacing the water heater. The quote follows by text.",
    },
    Call {
        id: "call-missed",
        name: "+13125550153",
        number: "+13125550153",
        direction: "inbound",
        minutes_ago: 1_290,
        seconds: 0,
        summary: "",
    },
    Call {
        id: "call-priya",
        name: "Priya Raman",
        number: "+13125550117",
        direction: "inbound",
        minutes_ago: 1_420,
        seconds: 220,
        summary: "Wants a quote for a bathroom remodel. Transferred to the office to book \
                  a site visit.",
    },
    Call {
        id: "call-daniel",
        name: "Daniel Osei",
        number: "+13125550131",
        direction: "inbound",
        minutes_ago: 2_880,
        seconds: 110,
        summary: "Low water pressure upstairs. Booked a visit for Monday morning.",
    },
    Call {
        id: "call-hannah",
        name: "Hannah Brooks",
        number: "+13125550176",
        direction: "inbound",
        minutes_ago: 3_020,
        seconds: 125,
        summary: "Outdoor faucet dripping since the frost. Sent a quote by text.",
    },
    Call {
        id: "call-marcus",
        name: "Marcus Bell",
        number: "+13125550108",
        direction: "inbound",
        minutes_ago: 4_400,
        seconds: 95,
        summary: "Asked for a copy of last month's invoice. Emailed it.",
    },
];

/// Rosa's call, as the call's own screen reads it: the log's row with what
/// the receptionist made of it.
fn rosa_call() -> Value {
    let mut call = serde_json::to_value(CALLS[0].summary()).expect("a call");
    call["sentiment"] = json!("positive");
    call["disposition"] = json!("booked");
    call["analysis"] = json!({
        "keyPoints": [
            "Leak under the kitchen sink since this morning",
            "The shut-off valve is closed for now"
        ],
        "objections": [],
        "topics": ["Repair", "Booking"],
        "actionItems": ["Bring a replacement trap and supply lines"],
        "followUpSuggested": false
    });
    call["followUp"] = json!({
        "email": null,
        "sms": FOLLOW_UP,
        "sentAt": ago(185)
    });
    call["phoneIntel"] = intel(CALLS[0].number);
    call
}

/// The text the receptionist sent Rosa after her call.
const FOLLOW_UP: &str = "Hi Rosa, this is Kestrel Row Plumbing. Your visit is booked for \
                         Thursday between 9 and 11 am. Reply here if anything changes.";

/// Rosa's call, as the service writes a transcript: a line a turn.
const TRANSCRIPT: &str = "\
Receptionist: Thanks for calling Kestrel Row Plumbing, this is Ava. How can I help?
Caller: Hi, my kitchen sink is leaking under the cabinet. It started this morning and \
it's getting worse.
Receptionist: I'm sorry to hear that. Have you been able to turn the water off?
Caller: Yes, I closed the valve under the sink, so it's stopped for now.
Receptionist: That was the right thing to do. A plumber can come on Thursday between \
9 and 11 am. Would that work?
Caller: Thursday morning is perfect.
Receptionist: You're booked in, and I'll text you the details now. Is there anything \
else I can help with?
Caller: No, that's everything. Thanks!";

/// What is known about `number`, a Chicago mobile number in E.164.
fn intel(number: &str) -> Value {
    let last4 = &number[number.len() - 4..];
    json!({
        "country": "US",
        "countryName": "United States",
        "nationalFormat": format!("(312) 555-{last4}"),
        "internationalFormat": format_phone_number(number),
        "region": {"code": "IL", "name": "Illinois", "city": "Chicago"},
        "lineType": "mobile",
        "carrier": null
    })
}

fn overview() -> Value {
    let recent: Vec<CallSummary> = CALLS.iter().take(5).map(Call::summary).collect();
    json!({
        "success": true,
        "workspaceId": WORKSPACE,
        "role": "client",
        "metrics": {
            "totalCalls": 1_284,
            "callsThisWeek": 63,
            "totalContacts": 342,
            "avgDuration": 154
        },
        "avgDurationLabel": "2m 34s",
        "recentCalls": recent
    })
}

/// A thread in the inbox's list.
fn conversation(
    contact: Option<(&str, &str)>,
    number: &str,
    last: (&str, &str, i64),
    unread: u32,
) -> Value {
    let digits = number.trim_start_matches('+');
    let (body, direction, minutes_ago) = last;
    json!({
        "key": digits,
        "threadKey": contact.map_or_else(
            || format!("addr:{digits}"),
            |(id, _)| format!("contact:{id}"),
        ),
        "counterpart": contact.map_or(number, |(_, name)| name),
        "matchKeys": [digits],
        "kind": "phone",
        "channels": ["sms"],
        "contactId": contact.map(|(id, _)| id),
        "contactName": contact.map(|(_, name)| name),
        "contactEmail": null,
        "contactPhone": digits,
        "canSms": true,
        "canEmail": false,
        "lastMessage": {
            "body": body,
            "direction": direction,
            "type": "sms",
            "status": if direction == "inbound" { "received" } else { "delivered" },
            "createdAt": ago(minutes_ago)
        },
        "unreadCount": unread,
        "totalMessages": 6
    })
}

fn conversations() -> Value {
    json!({
        "success": true,
        "conversations": [
            conversation(
                Some(("contact-rosa", "Rosa Delgado")),
                "+13125550142",
                (
                    "Perfect, see you Thursday. Should I clear out the cabinet first?",
                    "inbound",
                    6,
                ),
                1,
            ),
            conversation(
                None,
                "+13125550189",
                ("Do you come out for emergencies on weekends?", "inbound", 35),
                1,
            ),
            conversation(
                Some(("contact-priya", "Priya Raman")),
                "+13125550117",
                (
                    "Thanks for the quote. Could you start the week after next?",
                    "inbound",
                    140,
                ),
                0,
            ),
            conversation(
                Some(("contact-owen", "Owen Barker")),
                "+13125550164",
                (
                    "We can install the new water heater on Monday morning. Does 8 am work?",
                    "outbound",
                    1_190,
                ),
                0,
            ),
            conversation(
                Some(("contact-daniel", "Daniel Osei")),
                "+13125550131",
                (
                    "Thanks, the pressure is back to normal upstairs.",
                    "inbound",
                    2_700,
                ),
                0,
            ),
        ],
        "scanned": 38,
        "scanLimit": 500
    })
}

/// Rosa's thread, oldest first: her call, the receptionist's text after it,
/// and the messages since.
fn rosa_thread() -> Value {
    let text = |id: &str, direction: &str, minutes_ago: i64, body: &str| {
        json!({
            "id": id,
            "type": "sms",
            "timestamp": ago(minutes_ago),
            "direction": direction,
            "body": body,
            "status": if direction == "inbound" { "received" } else { "delivered" }
        })
    };
    json!({
        "success": true,
        "timeline": [
            {
                "id": "call-rosa",
                "type": "call",
                "timestamp": ago(190),
                "direction": "inbound",
                "body": CALLS[0].summary,
                "status": "completed",
                "duration": CALLS[0].seconds,
                "summary": CALLS[0].summary,
                "hasTranscript": true
            },
            text(
                "sms-1",
                "outbound",
                185,
                FOLLOW_UP,
            ),
            text("sms-2", "inbound", 41, "Thank you! Is there a charge for the visit itself?"),
            text(
                "sms-3",
                "outbound",
                30,
                "The service call fee is $79, and it comes off the bill if you go ahead \
                 with the repair.",
            ),
            text(
                "sms-4",
                "inbound",
                6,
                "Perfect, see you Thursday. Should I clear out the cabinet first?",
            ),
        ],
        "pageInfo": {"hasMore": false, "oldest": ago(190), "oldestId": "call-rosa"}
    })
}

/// A contact, with what the service keeps for them: `email` says whether
/// they gave an address, made from their name.
fn contact(name: &str, number: &str, email: bool, context: Option<&str>) -> Value {
    let first = name.split(' ').next().expect("a name").to_lowercase();
    let address = email.then(|| format!("{}@example.com", name.to_lowercase().replace(' ', ".")));
    json!({
        "id": format!("contact-{first}"),
        "workspaceId": WORKSPACE,
        "name": name,
        "phoneNumber": number,
        "email": address,
        "socialHandles": null,
        "company": null,
        "intelligence": null,
        "visualMemory": null,
        "latestContextSummary": context,
        "dgiStatus": null,
        "dgiError": null,
        "budget": null,
        "timeline": null,
        "website": null,
        "lastUpdated": ago(1_400),
        "createdAt": ago(86_000)
    })
}

fn contacts() -> ContactListResponse {
    let owen = "Water heater is 14 years old and losing pressure. Quoted for a \
                replacement, to be installed on Monday at 8 am.";
    let priya = "Remodeling the upstairs bathroom: a new shower valve, vanity and \
                 toilet. Wants to start the week after next.";
    let rosa = "Kitchen sink leak. Visit booked for Thursday between 9 and 11 am.";
    from(json!({
        "success": true,
        "contacts": [
            contact("Daniel Osei", "+13125550131", true, None),
            contact("Hannah Brooks", "+13125550176", false, None),
            contact("Marcus Bell", "+13125550108", true, None),
            contact("Owen Barker", "+13125550164", true, Some(owen)),
            contact("Priya Raman", "+13125550117", true, Some(priya)),
            contact("Rosa Delgado", "+13125550142", true, Some(rosa)),
            contact("Tomas Lindqvist", "+13125550195", true, None),
        ],
        "total": 7,
        "limit": 100,
        "offset": 0
    }))
}

/// The contact at `index` in [`contacts`], as its own screen reads it.
fn contact_detail(index: usize) -> ContactDetailResponse {
    let listed = contacts().contacts.swap_remove(index);
    let number = listed.phone_number.clone().expect("a number");
    from(json!({"success": true, "contact": listed, "phoneIntel": intel(&number)}))
}

/// The app, started with no session answered yet, in a window of the scenes'
/// size.
fn start() -> Store {
    let shots = std::env::var_os("DISTRICT_STORE_SHOTS").map(PathBuf::from);
    if let Some(dir) = &shots {
        fs::create_dir_all(dir).expect("the screenshot directory");
    }
    gtk::init().expect("a display: run this under xvfb-run, as the module says");
    // GNOME's defaults where Xvfb has none: its interface font as this
    // environment has it (Cantarell, GNOME's before Adwaita Sans, which is
    // not packaged here), a close button alone in the header bar, and a
    // cursor that does not blink. Transitions finish at once, so each
    // picture shows where the screen ends up.
    let settings = gtk::Settings::default().expect("settings for the display");
    settings.set_gtk_font_name(Some("Cantarell 11"));
    settings.set_gtk_decoration_layout(Some("appmenu:close"));
    settings.set_gtk_cursor_blink(false);
    settings.set_gtk_enable_animations(false);
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
    let script = Rc::new(Script::default());
    let (events, receiver) = async_channel::unbounded();
    let (commands, command_receiver) = async_channel::unbounded();
    let app = application(Parts {
        config: CoreConfig {
            web_base_url: "https://www.distronode.com".to_owned(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            // As the packages are built, with the `voice` feature: the script
            // plays the call engine.
            calls_available: true,
        },
        device_name: "Linux".to_owned(),
        effects: Rc::clone(&script) as Rc<dyn Effects>,
        events: (events.clone(), receiver),
        commands: command_receiver,
        startup_notice: None,
    });
    // Never the running app's own instance, whatever is on this session bus.
    app.set_flags(app.flags() | gio::ApplicationFlags::NON_UNIQUE);
    app.register(gio::Cancellable::NONE)
        .expect("the application registers");
    app.activate();
    let store = Store {
        app,
        script,
        events,
        _bridge: UiBridge::new(commands),
        shots,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !store.window().is_mapped() {
        assert!(Instant::now() < deadline, "the window never appeared");
        store.pump();
    }
    // The size every scene is drawn at. GTK takes the default size when a
    // hidden window is shown again.
    let window = store.window();
    window.remove_css_class("solid-csd");
    window.add_css_class("csd");
    window.set_visible(false);
    window.set_default_size(WIDTH, HEIGHT);
    window.present();
    store.pump();
    store
}

/// Signed in, from a stored session, to the business's one workspace, with
/// its overview.
fn overview_scene(store: &Store) {
    store.reply("RestoreSession", |ticket| Event::SessionRestored {
        ticket,
        result: Ok(AccessClaims {
            user_id: "user-store".to_owned(),
            device_id: "device-store".to_owned(),
            expires_at_secs: 4_000_000_000,
        }),
    });
    store.reply("ReadRingSetting", |ticket| Event::RingSettingRead {
        ticket,
        ring_here: false,
    });
    store.reply("LoadWorkspaces", |ticket| Event::WorkspacesLoaded {
        ticket,
        remembered: Some(WORKSPACE.to_owned()),
        result: Ok(from(json!({
            "success": true,
            "workspaces": [{
                "id": WORKSPACE,
                "name": BUSINESS,
                "region": "us",
                "role": "client",
                "subscriptionTier": "VoicePro"
            }],
            "total": 1,
            "limit": 100,
            "offset": 0,
            "degradedRegions": [],
            "inactiveCount": 0,
            "defaultWorkspaceId": WORKSPACE
        }))),
    });
    store.reply("LoadOverview", |ticket| Event::OverviewLoaded {
        ticket,
        result: Ok(from(overview())),
    });
    // Set up and answering: no card asking to finish on the web.
    store.reply("LoadSetupStatus", |ticket| Event::SetupStatusLoaded {
        ticket,
        result: Ok(false),
    });
    store.reply("LoadUnreadCount", |ticket| Event::UnreadCountLoaded {
        ticket,
        result: Ok(from(
            json!({"success": true, "count": 2, "workspaceId": WORKSPACE}),
        )),
    });
    assert!(store.shows(BUSINESS));
    assert!(store.shows("Rosa Delgado"), "the latest calls are listed");
    store.forget();
    store.shot("overview");
}

/// The inbox, with Rosa's thread open and a reply being written.
fn inbox_scene(store: &Store) {
    store.activate("sidebar-inbox", 0);
    store.reply("LoadConversations", |ticket| Event::ConversationsLoaded {
        ticket,
        result: Ok(from(conversations())),
    });
    store.reply("LoadDraftKeys", |ticket| Event::DraftKeysLoaded {
        ticket,
        result: Ok(from(json!({"success": true, "drafts": []}))),
    });
    store.activate("conversation-row", 0);
    store.reply("LoadTimeline", |ticket| Event::TimelineLoaded {
        ticket,
        result: Ok(from(rosa_thread())),
    });
    store.reply("LoadDraft {", |ticket| Event::DraftLoaded {
        ticket,
        result: Ok(from(json!({"success": true, "draft": null}))),
    });
    store.reply("MarkRead", |ticket| Event::MarkedRead {
        ticket,
        result: Ok(from(json!({"success": true, "marked": 1}))),
    });
    if store.pending("LoadUnreadCount") {
        store.reply("LoadUnreadCount", |ticket| Event::UnreadCountLoaded {
            ticket,
            result: Ok(from(
                json!({"success": true, "count": 1, "workspaceId": WORKSPACE}),
            )),
        });
    }
    store
        .one("text")
        .downcast::<gtk::TextView>()
        .expect("the composer")
        .buffer()
        .set_text("Yes please, that gives us room to work. See you Thursday!");
    store.pump();
    assert!(store.shows("Rosa Delgado"));
    store.forget();
    store.shot("inbox");
}

/// The call log, with Rosa's call open: its summary and transcript.
fn call_log_scene(store: &Store) {
    store.activate("sidebar-calls", 0);
    store.reply("LoadCalls", |ticket| Event::CallsLoaded {
        ticket,
        result: Ok(CALLS.iter().map(Call::summary).collect()),
    });
    store.activate("call-row", 0);
    store.reply("LoadCall {", |ticket| Event::CallLoaded {
        ticket,
        result: Ok(from(json!({"success": true, "call": rosa_call()}))),
    });
    store.reply("LoadTranscript", |ticket| Event::TranscriptLoaded {
        ticket,
        result: Ok(from(json!({"success": true, "transcript": TRANSCRIPT}))),
    });
    assert!(store.shows(TRANSCRIPT));
    store.forget();
    store.shot("call-log");
}

/// Contacts, with Priya's open.
fn contacts_scene(store: &Store) {
    store.activate("sidebar-contacts", 0);
    store.reply("LoadContacts", |ticket| Event::ContactsLoaded {
        ticket,
        result: Ok(contacts()),
    });
    store.activate("contact-row", 4);
    store.reply("LoadContact {", |ticket| Event::ContactLoaded {
        ticket,
        result: Ok(contact_detail(4)),
    });
    assert!(store.shows("Priya Raman"));
    store.forget();
    store.shot("contacts");
}

/// A call placed from the dialler to Owen, a minute and a quarter in, under
/// his contact.
fn call_scene(store: &Store) {
    store.activate("sidebar-calls", 0);
    store.forget();
    store.click("dial_button");
    store
        .one("number_entry")
        .downcast::<gtk::Entry>()
        .expect("the number box")
        .set_text("+1 312 555 0164");
    store.pump();
    store.click("call_button");
    store.reply("Dial", |ticket| Event::Dialled {
        ticket,
        result: Ok(from(json!({
            "success": true,
            "callId": "call-store-owen",
            "roomName": "direct_ws-kestrel-row-call-store-owen",
            "token": "store-screenshot-token",
            "url": "wss://media.example.com"
        }))),
    });
    let Effect::ConnectMedia { session, .. } = store.take("ConnectMedia") else {
        unreachable!()
    };
    let media = |event| {
        store.answer(Event::Media(MediaUpdate { session, event }));
    };
    media(MediaEvent::Connecting);
    media(MediaEvent::Connected);
    media(MediaEvent::ParticipantJoined(Participant::new(
        "sip_callee",
        None,
        false,
    )));
    media(MediaEvent::Microphone(MicrophoneState::On));
    // The clock, a second at a time, as the runner's waits end.
    let context = glib::MainContext::default();
    for _ in 0..75 {
        let mut pending = store.script.pending.borrow_mut();
        let index = pending
            .iter()
            .position(|effect| matches!(effect, Effect::Wait { delay, .. } if *delay == CALL_TICK))
            .expect("the call's clock is waiting");
        let tick = ticket(&pending.remove(index).expect("the index is in range"));
        drop(pending);
        store
            .events
            .try_send(Event::WaitOver { ticket: tick })
            .expect("the app is reading");
        while context.iteration(false) {}
    }
    store.pump();
    assert!(
        store.shows("01:15"),
        "the call has run a minute and a quarter"
    );
    store.activate("sidebar-contacts", 0);
    if store.pending("LoadContacts") {
        store.reply("LoadContacts", |ticket| Event::ContactsLoaded {
            ticket,
            result: Ok(contacts()),
        });
    }
    store.activate("contact-row", 3);
    store.reply("LoadContact {", |ticket| Event::ContactLoaded {
        ticket,
        result: Ok(contact_detail(3)),
    });
    assert!(store.shows("Owen Barker"));
    store.shot("call");
}

fn main() {
    let store = start();
    overview_scene(&store);
    inbox_scene(&store);
    call_log_scene(&store);
    contacts_scene(&store);
    call_scene(&store);
    store.app.emit_by_name::<()>("shutdown", &[]);
    println!("store screenshots drawn");
}
