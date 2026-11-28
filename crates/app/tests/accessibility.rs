//! Screen reader and keyboard checks on the real app, with synthetic demo data and no window.

use app::demo::{self, DemoConfig, DemoSetup};
use app::screenshot::CaptureView;
use app::settings::Theme;
use app::theme;
use app::ui::PhotosortApp;
use app::views::gallery;
use app::worker::NoThumbnails;
use eframe::App as _;
use eframe::egui::accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId};
use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, Shape, Vec2};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

const SCREEN: Vec2 = Vec2::new(1280.0, 820.0);

fn demo_setup() -> &'static DemoSetup {
    static SETUP: OnceLock<DemoSetup> = OnceLock::new();
    SETUP.get_or_init(|| {
        demo::prepare_demo(
            None,
            DemoConfig {
                count: 40,
                seed: demo::DEFAULT_SEED,
            },
            None,
        )
        .expect("datos de demostración")
    })
}

struct Harness {
    ctx: egui::Context,
    app: PhotosortApp,
    frame: eframe::Frame,
    nodes: HashMap<NodeId, Node>,
    focus: NodeId,
    shapes: Vec<Shape>,
}

impl Harness {
    fn new(view: CaptureView, theme: Theme) -> Self {
        Self::open(demo_setup(), view, theme)
    }

    fn open(setup: &DemoSetup, view: CaptureView, theme: Theme) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = PhotosortApp::new_with_mode(&cc, setup.data_dir.clone(), true, |_, _| {
            (Arc::new(NoThumbnails), None)
        })
        .with_initial_view(view, None)
        .with_theme_override(&ctx, theme);
        let mut harness = Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            nodes: HashMap::new(),
            focus: NodeId(0),
            shapes: Vec::new(),
        };
        harness.settle();
        harness
    }

    fn run(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        };
        let (app, frame) = (&mut self.app, &mut self.frame);
        let mut out = self.ctx.run_ui(input, |ui| app.ui(ui, frame));
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update.expect("árbol");
        self.focus = update.focus;
        self.nodes = update.nodes.into_iter().collect();
        self.shapes = out.shapes.into_iter().map(|s| s.shape).collect();
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.run(Vec::new());
        }
    }

    fn press(&mut self, key: Key, modifiers: Modifiers) {
        let event = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.run(vec![event(true)]);
        self.run(vec![event(false)]);
        self.settle();
    }

    /// What a screen reader would call the node.
    fn name(&self, node: &Node) -> String {
        if let Some(label) = node.label() {
            return label.to_owned();
        }
        node.labelled_by()
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter_map(|n| n.label().or_else(|| n.value()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn focused_name(&self) -> String {
        self.nodes
            .get(&self.focus)
            .map(|n| self.name(n))
            .unwrap_or_default()
    }

    fn find(&self, role: Role, name: &str) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|(_, n)| n.role() == role && self.name(n) == name)
            .map(|(id, _)| *id)
    }

    fn has_name_containing(&self, text: &str) -> bool {
        self.nodes.values().any(|n| self.name(n).contains(text))
    }

    /// What a screen reader reads out of plain text, which is a value rather than a name.
    fn has_text_containing(&self, text: &str) -> bool {
        self.nodes
            .values()
            .any(|n| self.name(n).contains(text) || n.value().is_some_and(|v| v.contains(text)))
    }

    /// Activates a control the way a screen reader does.
    fn click(&mut self, name: &str) {
        self.activate(Role::Button, name);
    }

    fn toggle(&mut self, name: &str) {
        self.activate(Role::CheckBox, name);
    }

    fn activate(&mut self, role: Role, name: &str) {
        let target = self
            .find(role, name)
            .unwrap_or_else(|| panic!("no hay {role:?} «{name}»"));
        self.run(vec![Event::AccessKitActionRequest(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: None,
        })]);
        self.settle();
    }

    fn unnamed_controls(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|n| {
                let control = matches!(
                    n.role(),
                    Role::Button
                        | Role::CheckBox
                        | Role::RadioButton
                        | Role::Switch
                        | Role::TextInput
                        | Role::MultilineTextInput
                        | Role::ComboBox
                        | Role::Slider
                        | Role::SpinButton
                        | Role::DisclosureTriangle
                );
                let focusable = n.supports_action(Action::Focus) && n.role() != Role::ScrollBar;
                // A bare symbol such as «✕» is not a name.
                (control || focusable) && !self.name(n).chars().any(char::is_alphanumeric)
            })
            .map(|n| {
                format!(
                    "{:?} «{}» (valor {:?}, ayuda {:?}) en {:?}",
                    n.role(),
                    self.name(n),
                    n.value(),
                    n.placeholder(),
                    n.bounds()
                )
            })
            .collect()
    }

    fn assert_all_named(&self, screen: &str) {
        let unnamed = self.unnamed_controls();
        assert!(
            unnamed.is_empty(),
            "{screen}: controles sin nombre:\n{}",
            unnamed.join("\n")
        );
    }
}

#[test]
fn every_control_in_every_view_has_a_name() {
    let mut failures = Vec::new();
    for view in [
        CaptureView::Gallery,
        CaptureView::Suggestions,
        CaptureView::Duplicates,
        CaptureView::Works,
        CaptureView::Persons,
        CaptureView::Tags,
        CaptureView::Errors,
        CaptureView::Settings,
        CaptureView::Welcome,
    ] {
        let harness = Harness::new(view, Theme::Dark);
        failures.extend(
            harness
                .unnamed_controls()
                .into_iter()
                .map(|c| format!("{view:?}: {c}")),
        );
    }
    assert!(failures.is_empty(), "sin nombre:\n{}", failures.join("\n"));

    // Gallery with a selection: selection bar, inspector, viewer, menu and export window.
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::ArrowRight, Modifiers::NONE);
    harness.assert_all_named("galería con selección");
    harness.press(Key::Enter, Modifiers::NONE);
    harness.assert_all_named("visor");
    harness.press(Key::Escape, Modifiers::NONE);
    harness.click("Copiar o mover…");
    harness.assert_all_named("exportar");
}

#[test]
fn thumbnails_say_their_file_and_state_and_switches_say_what_they_control() {
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::A, Modifiers::COMMAND);
    let thumbnails: Vec<String> = harness
        .nodes
        .values()
        .filter(|n| n.role() == Role::Button && n.toggled().is_some())
        .map(|n| harness.name(n))
        .filter(|name| is_photo(name))
        .collect();
    assert!(!thumbnails.is_empty(), "no hay miniaturas en el árbol");
    for state in [", revisada", ", con sugerencias"] {
        assert!(
            thumbnails.iter().any(|n| n.contains(state)),
            "ninguna miniatura dice «{state}»: {thumbnails:?}"
        );
    }

    let harness = Harness::new(CaptureView::Settings, Theme::Dark);
    for title in [
        "Revisar fotos en segundo plano",
        "Reglas rápidas",
        "Reconocer imágenes",
        "Leer texto",
        "Agrupar rostros",
    ] {
        assert!(harness.find(Role::Switch, title).is_some(), "{title}");
    }
    let chosen_theme = harness
        .find(Role::RadioButton, "Oscuro")
        .expect("opción de tema");
    assert_eq!(
        harness.nodes[&chosen_theme].toggled(),
        Some(egui::accesskit::Toggled::True)
    );
}

/// Names reached by pressing Tab from the start until the focus comes round.
fn tab_stops(harness: &mut Harness) -> Vec<String> {
    let mut stops = Vec::new();
    let mut seen = Vec::new();
    for _ in 0..400 {
        harness.press(Key::Tab, Modifiers::NONE);
        if seen.first() == Some(&harness.focus) {
            break;
        }
        seen.push(harness.focus);
        stops.push(harness.focused_name());
        let bounds = harness
            .nodes
            .get(&harness.focus)
            .and_then(|n| n.bounds())
            .unwrap_or_else(|| panic!("el foco no tiene posición: {:?}", harness.focus));
        let center = (bounds.y0 + bounds.y1) / 2.0;
        assert!(
            (0.0..=f64::from(SCREEN.y)).contains(&center),
            "el foco quedó fuera de la vista en «{}»: {bounds:?}",
            harness.focused_name()
        );
    }
    stops
}

#[test]
fn tab_reaches_the_main_actions_and_keeps_the_focus_in_view() {
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::ArrowRight, Modifiers::NONE);
    let stops = tab_stops(&mut harness);
    for wanted in [
        "Buscar en texto o ruta",
        "Configuración",
        "Copiar o mover…",
        "Quitar selección",
        "Visor",
        ".png",
    ] {
        assert!(
            stops.iter().any(|s| s.contains(wanted)),
            "Tab no llega a «{wanted}»: {stops:#?}"
        );
    }

    let mut harness = Harness::new(CaptureView::Settings, Theme::Dark);
    let stops = tab_stops(&mut harness);
    for wanted in ["Añadir carpeta…", "Agrupar rostros", "Claro"] {
        assert!(
            stops.iter().any(|s| s.contains(wanted)),
            "Tab no llega a «{wanted}» en Configuración: {stops:#?}"
        );
    }
}

fn is_photo(name: &str) -> bool {
    name.contains(".jpg") || name.contains(".png")
}

#[test]
fn menu_keys_open_the_photo_menu_and_escape_closes_menus_and_dialogs() {
    const FIRST_ITEM: &str = "Abrir en el visor";
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::ArrowRight, Modifiers::NONE);

    for (key, modifiers) in [(Key::Menu, Modifiers::NONE), (Key::F10, Modifiers::SHIFT)] {
        harness.press(key, modifiers);
        assert_eq!(harness.focused_name(), FIRST_ITEM, "{key:?}");
        harness.press(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(harness.focused_name(), "Mostrar en el Explorador");
        harness.press(Key::Escape, Modifiers::NONE);
        assert!(!harness.has_name_containing(FIRST_ITEM), "{key:?}");
    }

    // With the focus on a thumbnail, the menu is for that thumbnail, and the focus goes back.
    for _ in 0..200 {
        harness.press(Key::Tab, Modifiers::NONE);
        if is_photo(&harness.focused_name()) {
            break;
        }
    }
    harness.press(Key::Tab, Modifiers::NONE);
    let thumbnail = harness.focused_name();
    assert!(is_photo(&thumbnail), "{thumbnail}");
    harness.press(Key::Menu, Modifiers::NONE);
    assert_eq!(harness.focused_name(), FIRST_ITEM);
    let focused = &harness.nodes[&harness.find(Role::Button, &thumbnail).expect("miniatura")];
    assert_eq!(focused.toggled(), Some(egui::accesskit::Toggled::True));
    harness.press(Key::Escape, Modifiers::NONE);
    assert_eq!(harness.focused_name(), thumbnail);

    harness.click("Copiar o mover…");
    assert!(harness.has_name_containing("Calcular vista previa"));
    // The focus starts inside the window, not behind it.
    assert_eq!(harness.focused_name(), "Carpeta de destino");
    harness.press(Key::Escape, Modifiers::NONE);
    assert!(!harness.has_name_containing("Calcular vista previa"));
    // Escape closed the window only: the selection is still there.
    assert!(harness.find(Role::Button, "Quitar selección").is_some());
}

#[test]
fn walking_the_whole_photo_menu_leaves_the_gallery_where_it_was() {
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::ArrowRight, Modifiers::NONE);
    for _ in 0..200 {
        harness.press(Key::Tab, Modifiers::NONE);
        if is_photo(&harness.focused_name()) {
            break;
        }
    }
    let thumbnail = harness.focused_name();
    let place = |h: &Harness| {
        h.find(Role::Button, &thumbnail)
            .and_then(|id| h.nodes[&id].bounds())
    };
    let before = place(&harness);
    harness.press(Key::Menu, Modifiers::NONE);
    // Past the last order and round to the first one again, and sideways.
    for key in [Key::ArrowDown, Key::Tab, Key::ArrowRight, Key::ArrowLeft] {
        for _ in 0..12 {
            harness.press(key, Modifiers::NONE);
            assert_eq!(
                place(&harness),
                before,
                "{key:?}: la galería se movió con el foco en «{}»",
                harness.focused_name()
            );
        }
    }
    harness.press(Key::Escape, Modifiers::NONE);
    assert_eq!(harness.focused_name(), thumbnail);
}

#[test]
fn renaming_a_person_puts_the_focus_in_the_name_field() {
    let mut harness = Harness::new(CaptureView::Persons, Theme::Dark);
    for _ in 0..200 {
        harness.press(Key::Tab, Modifiers::NONE);
        if harness.focused_name() == "Renombrar" {
            break;
        }
    }
    harness.press(Key::Enter, Modifiers::NONE);
    assert_eq!(harness.focused_name(), "Nombre de la persona");
}

#[test]
fn keyboard_focus_ring_is_drawn_in_both_themes() {
    for (preference, palette) in [
        (Theme::Dark, &theme::DARK_PALETTE),
        (Theme::Light, &theme::LIGHT_PALETTE),
    ] {
        let mut harness = Harness::new(CaptureView::Settings, preference);
        // Includes buttons that set their own stroke, which hides egui's focus stroke.
        for _ in 0..30 {
            harness.press(Key::Tab, Modifiers::NONE);
            let bounds = harness.nodes[&harness.focus].bounds().expect("posición");
            let ringed = harness.shapes.iter().any(|shape| {
                matches!(shape, Shape::Rect(r)
                    if r.stroke.color == palette.accent.color32()
                        && r.stroke.width == theme::FOCUS_STROKE_WIDTH
                        && r.stroke_kind == egui::StrokeKind::Outside
                        && f64::from(r.rect.center().x) >= bounds.x0
                        && f64::from(r.rect.center().x) <= bounds.x1)
            });
            assert!(
                ringed,
                "{preference:?}: sin borde de foco en «{}»",
                harness.focused_name()
            );
        }
    }
}

fn enter_key(pressed: bool) -> Event {
    Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

impl Harness {
    fn focus_on(&mut self, role: Role, name: &str) {
        let target = self
            .find(role, name)
            .unwrap_or_else(|| panic!("no hay «{name}»"));
        self.run(vec![Event::AccessKitActionRequest(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: None,
        })]);
        assert_eq!(self.focused_name(), name);
    }

    /// Runs frames, while `held` with Enter down and repeating, until `done` or a timeout.
    fn wait_for(&mut self, held: bool, done: impl Fn(&Self) -> bool) -> bool {
        for _ in 0..500 {
            if done(self) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            self.run(if held {
                vec![enter_key(true)]
            } else {
                Vec::new()
            });
        }
        false
    }

    /// Opens the export window for the selection, types `dest` and presses Enter to preview it.
    /// With `hold`, Enter stays down and repeats once the preview is ready.
    fn export_preview(&mut self, dest: &std::path::Path, hold: bool) {
        self.click("Copiar o mover…");
        assert_eq!(self.focused_name(), "Carpeta de destino");
        self.press(Key::A, Modifiers::COMMAND);
        self.run(vec![Event::Text(dest.display().to_string())]);
        self.run(vec![enter_key(true)]);
        let ready = self.wait_for(hold, |h| h.find(Role::Button, "Copiar").is_some());
        assert!(ready, "la vista previa no terminó");
        if hold {
            for _ in 0..10 {
                self.run(vec![enter_key(true)]);
            }
        }
        self.run(vec![enter_key(false)]);
        self.settle();
    }
}

fn is_empty_dir(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir).expect("carpeta").next().is_none()
}

#[test]
fn enter_on_close_or_back_never_starts_an_export() {
    let out = tempfile::tempdir().expect("carpeta temporal");
    let dest = out.path().join("copias");
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::ArrowRight, Modifiers::NONE);

    for button in ["Cerrar", "Volver"] {
        harness.export_preview(&dest, false);
        harness.focus_on(Role::Button, button);
        harness.press(Key::Enter, Modifiers::NONE);
        assert!(harness.find(Role::Button, "Copiar").is_none(), "{button}");
        assert!(is_empty_dir(out.path()), "Intro en «{button}» exportó");
        // «Volver» leaves the window open on the first step.
        let open = harness.has_name_containing("Calcular vista previa");
        assert_eq!(open, button == "Volver", "{button}");
        if open {
            harness.press(Key::Escape, Modifiers::NONE);
        }
    }

    // Enter held from the destination field repeats into the ready preview: no export.
    harness.export_preview(&dest, true);
    assert!(harness.find(Role::Button, "Copiar").is_some());
    assert!(is_empty_dir(out.path()), "Intro mantenido exportó");

    // A fresh Enter with no control focused is still the default action.
    harness.press(Key::Enter, Modifiers::NONE);
    let done = harness.wait_for(false, |h| h.has_name_containing("Abrir carpeta"));
    assert!(done && !is_empty_dir(&dest), "Intro sin foco no exportó");
}

/// Moving asks twice, and Enter never answers the second question.
#[test]
fn enter_never_confirms_a_move() {
    let out = tempfile::tempdir().expect("carpeta temporal");
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::ArrowRight, Modifiers::NONE);
    harness.click("Copiar o mover…");
    harness.activate(Role::RadioButton, "Mover (cortar)");
    harness.assert_all_named("mover");
    harness.focus_on(Role::TextInput, "Carpeta de destino");
    harness.press(Key::A, Modifiers::COMMAND);
    harness.run(vec![Event::Text(out.path().display().to_string())]);
    harness.press(Key::Enter, Modifiers::NONE);
    assert!(harness.wait_for(false, |h| h.find(Role::Button, "Mover…").is_some()));

    harness.press(Key::Enter, Modifiers::NONE);
    assert!(harness.has_text_containing("Puedes deshacerlo."));
    harness.assert_all_named("confirmar el movimiento");
    for _ in 0..3 {
        harness.press(Key::Enter, Modifiers::NONE);
    }
    assert!(harness.find(Role::Button, "Mover").is_some());
    assert!(is_empty_dir(out.path()), "Intro movió la foto");

    harness.click("Volver");
    assert!(harness.find(Role::Button, "Mover…").is_some());

    // Enter held on the focused «Mover…» repeats into the confirmation, whose button takes its
    // place: the repeats must not press it.
    harness.focus_on(Role::Button, "Mover…");
    for _ in 0..10 {
        harness.run(vec![enter_key(true)]);
    }
    harness.run(vec![enter_key(false)]);
    harness.settle();
    std::thread::sleep(std::time::Duration::from_millis(300));
    harness.settle();
    assert!(harness.has_text_containing("Puedes deshacerlo."));
    assert!(harness.find(Role::Button, "Mover").is_some());
    assert!(is_empty_dir(out.path()), "Intro mantenido movió la foto");
}

#[test]
fn reload_and_undo_have_names_and_are_reached_with_tab() {
    // Its own data: the test writes to the catalog, so that there is something to undo.
    let config = DemoConfig {
        count: 8,
        seed: demo::DEFAULT_SEED,
    };
    let setup = demo::prepare_demo(None, config, None).expect("datos de demostración");
    let harness = Harness::open(&setup, CaptureView::Gallery, Theme::Dark);
    let undo = harness
        .find(Role::Button, "Deshacer último movimiento")
        .expect("botón");
    assert!(
        harness.nodes[&undo].is_disabled(),
        "todavía no hay nada que deshacer"
    );

    let db = app::settings::db_dest_path(&setup.data_dir, &[]).expect("ruta del catálogo");
    let cat = catalog::Catalog::open(&db).expect("catálogo");
    // A logged move of a file that is in no library: only the log changes.
    let nowhere = setup.data_dir.join("no-existe");
    let id = cat
        .begin_move(&catalog::NewMove {
            batch: 1,
            src_path: &nowhere,
            src_root: &nowhere,
            dst_path: &nowhere,
            dst_root: None,
            sha256: "0",
            size: 0,
            mtime_ns: None,
            method: catalog::MoveMethod::Rename,
        })
        .expect("registro");
    cat.finish_move(id, catalog::MoveState::Done, None)
        .expect("hecho");

    let mut harness = Harness::open(&setup, CaptureView::Gallery, Theme::Dark);
    harness.assert_all_named("barra de la galería");
    let stops = tab_stops(&mut harness);
    for wanted in ["Recargar", "Deshacer último movimiento"] {
        assert!(
            stops.iter().any(|s| s == wanted),
            "Tab no llega a «{wanted}»: {stops:#?}"
        );
    }
}

#[test]
fn names_that_egui_gives_are_in_spanish() {
    let mut harness = Harness::new(CaptureView::Gallery, Theme::Dark);
    harness.press(Key::ArrowRight, Modifiers::NONE);
    assert!(
        harness
            .find(Role::Splitter, "Cambiar el tamaño del panel")
            .is_some()
    );
    harness.click("Copiar o mover…");
    assert!(harness.find(Role::Button, "Cerrar ventana").is_some());
    for english in ["Close window", "Resize panel"] {
        assert!(!harness.has_name_containing(english), "{english}");
    }
}

#[test]
fn persons_view_reuses_what_it_loaded_until_the_app_changes_the_catalog() {
    // Its own data: the test writes to the catalog.
    let config = DemoConfig {
        count: 40,
        seed: demo::DEFAULT_SEED,
    };
    let setup = demo::prepare_demo(None, config, None).expect("datos de demostración");
    let db = app::settings::db_dest_path(&setup.data_dir, &[]).expect("ruta del catálogo");
    let mut cat = catalog::Catalog::open(&db).expect("catálogo");

    // The demo's unnamed person is in two photos; the naming card asks from three.
    let roots = app::settings::load(&setup.data_dir).settings.library_roots;
    let persons = cat.list_persons_in_roots(true, &roots).expect("personas");
    let unnamed = persons
        .iter()
        .find(|p| p.name.is_none())
        .expect("sin nombre");
    let alex = persons
        .iter()
        .find(|p| p.name.as_deref() == Some("Alex"))
        .expect("Alex");
    let face = cat.person_faces_in_roots(alex.id, &roots).expect("rostros")[0].id;
    cat.assign_face_to_person(face, unnamed.id, false)
        .expect("asignar");

    let mut harness = Harness::open(&setup, CaptureView::Persons, Theme::Dark);
    assert!(harness.has_name_containing("Misma persona: Alex"));

    // A write behind the app's back: drawing again must not read it.
    let alex = cat
        .find_person_by_name("Alex")
        .expect("consulta")
        .expect("Alex");
    cat.rename_person(alex.id, Some("Renombrada por fuera"))
        .expect("renombrar");
    harness.settle();
    assert!(harness.has_name_containing("Misma persona: Alex"));
    assert!(!harness.has_name_containing("Renombrada por fuera"));

    // «Más tarde» takes the naming card away at once.
    assert!(harness.find(Role::Button, "Más tarde").is_some());
    harness.click("Más tarde");
    assert!(harness.find(Role::Button, "Más tarde").is_none());

    // A change made in the view reloads: the merged card is gone and the new name is read.
    harness.toggle("Misma persona: Alex");
    harness.toggle("Misma persona: Sin nombre");
    harness.click("Unir");
    assert!(harness.has_name_containing("Misma persona: Renombrada por fuera"));
    assert!(!harness.has_name_containing("Misma persona: Sin nombre"));
}

impl Harness {
    /// Clicks a button with the mouse, the way a user does, at the centre of its box.
    fn mouse_click(&mut self, name: &str) {
        // The topmost one that is on screen: a click outside the window does nothing.
        let at = self
            .nodes
            .values()
            .filter(|n| n.role() == Role::Button && self.name(n) == name)
            .filter_map(|n| n.bounds())
            .map(|b| Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32))
            .filter(|p| p.x < SCREEN.x && p.y < SCREEN.y)
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .unwrap_or_else(|| panic!("no hay botón «{name}» a la vista"));
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.run(vec![Event::PointerMoved(at)]);
        self.run(vec![button(true)]);
        self.run(vec![button(false)]);
        self.settle();
    }
}

#[test]
fn the_remove_folder_button_works_with_the_mouse() {
    // Its own demo: removing the folder must not affect the other tests.
    let setup = demo::prepare_demo(
        None,
        DemoConfig {
            count: 12,
            seed: demo::DEFAULT_SEED,
        },
        None,
    )
    .expect("datos de demostración");
    let mut h = Harness::open(&setup, CaptureView::Settings, Theme::Dark);
    h.mouse_click("Quitar");
    assert!(
        h.find(Role::Button, "Quitar de la app").is_some(),
        "«Quitar» no abrió la confirmación"
    );
    h.mouse_click("Quitar de la app");
    assert!(
        h.find(Role::Button, "Quitar").is_none(),
        "la carpeta sigue en la lista"
    );
}

#[test]
fn row_buttons_get_their_own_clicks_in_suggestions_and_tags() {
    // Accepting a suggestion with the mouse removes it from the list.
    let mut h = Harness::new(CaptureView::Suggestions, Theme::Dark);
    let accepts = |h: &Harness| {
        h.nodes
            .values()
            .filter(|n| n.role() == Role::Button && h.name(n) == "Aceptar")
            .count()
    };
    let before = accepts(&h);
    assert!(before > 0, "no hay sugerencias en la demo");
    h.mouse_click("Aceptar");
    assert!(accepts(&h) < before, "«Aceptar» no respondió al ratón");

    // «Acciones» opens its menu with the mouse.
    let mut h = Harness::new(CaptureView::Tags, Theme::Dark);
    h.mouse_click("Acciones");
    assert!(
        h.find(Role::Button, "Renombrar").is_some()
            || h.find(Role::MenuItem, "Renombrar").is_some(),
        "«Acciones» no abrió su menú"
    );
}

#[test]
fn removing_the_only_folder_of_a_filtered_person_drops_the_filter() {
    // Its own demo plus a second folder holding a copy of one faceless photo.
    let setup = demo::prepare_demo(
        None,
        DemoConfig {
            count: 40,
            seed: demo::DEFAULT_SEED,
        },
        None,
    )
    .expect("datos de demostración");
    let db = app::settings::db_dest_path(&setup.data_dir, &[]).expect("ruta del catálogo");
    let mut cat = catalog::Catalog::open(&db).expect("catálogo");
    let faceless = walkdir::WalkDir::new(&setup.library_dir)
        .into_iter()
        .map(|e| e.expect("entrada").into_path())
        .filter(|p| p.is_file())
        .find(|p| {
            let canonical = dunce::canonicalize(p).expect("ruta");
            cat.asset_of(&canonical)
                .expect("consulta")
                .is_some_and(|a| cat.asset_face_details(a).expect("caras").is_empty())
        })
        .expect("foto sin caras");
    let extra = tempfile::tempdir().expect("segunda carpeta");
    let name = faceless.file_name().expect("nombre").to_owned();
    // A fixture: a copy of a demo photo into a temp dir, never into a library root.
    #[allow(clippy::disallowed_methods)]
    std::fs::copy(&faceless, extra.path().join(&name)).expect("copia");
    let extra_root = fsguard::LibraryRoot::new(extra.path()).expect("raíz");
    cat.index(&extra_root).expect("indexar");
    let mut settings = app::settings::load(&setup.data_dir).settings;
    settings
        .add_library_root(extra.path(), &setup.data_dir)
        .expect("añadir carpeta");
    app::settings::save(&setup.data_dir, &settings).expect("guardar ajustes");
    let copy = name.to_string_lossy().into_owned();

    // Alex only appears in the demo folder; filtering by Alex hides the copy.
    let mut h = Harness::open(&setup, CaptureView::Gallery, Theme::Dark);
    let header = h
        .nodes
        .values()
        .find(|n| h.name(n) == "Personas (2)")
        .map(|n| n.role())
        .expect("grupo Personas");
    h.activate(header, "Personas (2)");
    let alex = h
        .nodes
        .values()
        .map(|n| h.name(n))
        .find(|n| n.starts_with("Alex ("))
        .expect("botón de Alex");
    h.click(&alex);
    let copy_shown = |h: &Harness| h.nodes.values().any(|n| h.name(n).starts_with(&copy));
    assert!(!copy_shown(&h), "el filtro por Alex no se aplicó");

    // Removing the demo folder takes Alex away, and his filter with him.
    h.click("Configuración");
    h.mouse_click("Quitar");
    h.mouse_click("Quitar de la app");
    h.click("Todas, 1");
    assert!(
        copy_shown(&h),
        "el filtro de una persona que ya no se ve sigue activo"
    );
}

impl Harness {
    fn click_at(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.run(vec![Event::PointerMoved(at)]);
        self.run(vec![button(true)]);
        self.run(vec![button(false)]);
        self.settle();
    }
}

#[test]
fn a_click_on_the_empty_grid_drops_the_selection() {
    // Two photos leave most of the grid empty. Its own data: nothing else needs so few.
    let config = DemoConfig {
        count: 2,
        seed: demo::DEFAULT_SEED,
    };
    let setup = demo::prepare_demo(None, config, None).expect("datos de demostración");
    let mut h = Harness::open(&setup, CaptureView::Gallery, Theme::Dark);
    h.press(Key::A, Modifiers::COMMAND);
    assert!(h.find(Role::Button, "Quitar selección").is_some());

    // The selection bar is not empty space.
    let bar = h
        .find(Role::Button, "Quitar selección")
        .and_then(|id| h.nodes[&id].bounds())
        .expect("barra");
    h.click_at(Pos2::new(
        bar.x1 as f32 + 40.0,
        ((bar.y0 + bar.y1) / 2.0) as f32,
    ));
    assert!(h.find(Role::Button, "Quitar selección").is_some());

    // Left of the inspector and below the single row of photos.
    h.click_at(Pos2::new(SCREEN.x / 2.0, SCREEN.y - 30.0));
    assert!(h.find(Role::Button, "Quitar selección").is_none());
}

#[test]
fn the_corner_circle_adds_and_removes_one_photo_and_keeps_the_others() {
    let mut h = Harness::new(CaptureView::Gallery, Theme::Dark);
    // The thumbnails in reading order, with whether each is selected.
    let cells = |h: &Harness| {
        let mut cells: Vec<(Rect, bool, String)> = h
            .nodes
            .values()
            .filter(|n| n.role() == Role::Button && is_photo(&h.name(n)))
            .filter_map(|n| {
                let b = n.bounds()?;
                let rect = Rect::from_min_max(
                    Pos2::new(b.x0 as f32, b.y0 as f32),
                    Pos2::new(b.x1 as f32, b.y1 as f32),
                );
                let selected = n.toggled() == Some(egui::accesskit::Toggled::True);
                Some((rect, selected, h.name(n)))
            })
            .collect();
        cells.sort_by(|a, b| {
            a.0.min
                .y
                .total_cmp(&b.0.min.y)
                .then(a.0.min.x.total_cmp(&b.0.min.x))
        });
        cells
    };
    let selected = |h: &Harness| cells(h).iter().take(2).map(|c| c.1).collect::<Vec<_>>();

    h.click_at(cells(&h)[0].0.center());
    assert_eq!(selected(&h), [true, false]);
    // The inspector has just opened and moved the grid, so each place is asked for again.
    h.click_at(gallery::check_rect(cells(&h)[1].0).center());
    assert_eq!(selected(&h), [true, true]);
    h.click_at(gallery::check_rect(cells(&h)[1].0).center());
    assert_eq!(selected(&h), [true, false]);
    assert!(
        !h.has_name_containing("Siguiente"),
        "el círculo abrió el visor"
    );

    // With the keyboard, Ctrl+Space on a focused thumbnail does the same.
    let name = cells(&h)[1].2.clone();
    h.focus_on(Role::Button, &name);
    // The window reports a held key on its own, apart from the key that follows.
    h.run(vec![Event::ModifiersChanged(Modifiers::COMMAND)]);
    h.press(Key::Space, Modifiers::COMMAND);
    assert_eq!(selected(&h), [true, true]);
}

#[test]
fn back_returns_from_a_persons_photos_to_persons() {
    const BACK: &str = "Volver a la vista anterior";
    let mut h = Harness::new(CaptureView::Persons, Theme::Dark);
    let on_persons = |h: &Harness| h.find(Role::Button, "Renombrar").is_some();
    assert!(on_persons(&h) && h.find(Role::Button, BACK).is_none());

    h.mouse_click("Ver fotos");
    assert!(!on_persons(&h));
    h.mouse_click(BACK);
    assert!(on_persons(&h) && h.find(Role::Button, BACK).is_none());

    // The same with Alt+←.
    h.mouse_click("Ver fotos");
    h.press(Key::ArrowLeft, Modifiers::ALT);
    assert!(on_persons(&h));

    // A section opened from the sidebar has nothing to go back to.
    h.mouse_click("Ver fotos");
    h.click("Configuración");
    assert!(h.find(Role::Button, BACK).is_none());

    // From the gallery to the similar ones and back: the same photos, selected and scrolled.
    let mut h = Harness::new(CaptureView::Gallery, Theme::Dark);
    h.press(Key::ArrowRight, Modifiers::NONE);
    let wheel = |h: &mut Harness, points: f32| {
        h.run(vec![
            Event::PointerMoved(Pos2::new(SCREEN.x / 2.0, SCREEN.y / 2.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, points),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::NONE,
            },
        ]);
        for _ in 0..40 {
            h.run(Vec::new());
        }
    };
    wheel(&mut h, -90.0);
    let shown = |h: &Harness| {
        let mut photos: Vec<String> = h
            .nodes
            .values()
            .filter(|n| n.role() == Role::Button && is_photo(&h.name(n)))
            .map(|n| format!("{} {:?} {:?}", h.name(n), n.toggled(), n.bounds()))
            .collect();
        photos.sort();
        photos
    };
    let before = shown(&h);
    h.press(Key::Menu, Modifiers::NONE);
    h.click("Buscar parecidas");
    assert_ne!(shown(&h), before);
    // Another selection and another place, so that only «Volver» can bring them back.
    h.press(Key::ArrowRight, Modifiers::NONE);
    wheel(&mut h, 2000.0);
    h.click(BACK);
    h.settle();
    assert_eq!(shown(&h), before);
}
