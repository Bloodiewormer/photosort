//! Screen reader and keyboard checks on the real app, with synthetic demo data and no window.

use app::demo::{self, DemoConfig, DemoSetup};
use app::screenshot::CaptureView;
use app::settings::Theme;
use app::theme;
use app::ui::PhotosortApp;
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
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = PhotosortApp::new_with_mode(&cc, demo_setup().data_dir.clone(), true, |_, _| {
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

    /// Activates a control the way a screen reader does.
    fn click(&mut self, name: &str) {
        let target = self
            .find(Role::Button, name)
            .unwrap_or_else(|| panic!("no hay botón «{name}»"));
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
    harness.click("Exportar…");
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
        "Exportar…",
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

    harness.click("Exportar…");
    assert!(harness.has_name_containing("Calcular vista previa"));
    // The focus starts inside the window, not behind it.
    assert_eq!(harness.focused_name(), "Carpeta de destino");
    harness.press(Key::Escape, Modifiers::NONE);
    assert!(!harness.has_name_containing("Calcular vista previa"));
    // Escape closed the window only: the selection is still there.
    assert!(harness.find(Role::Button, "Quitar selección").is_some());
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
        self.click("Exportar…");
        assert_eq!(self.focused_name(), "Carpeta de destino");
        self.press(Key::A, Modifiers::COMMAND);
        self.run(vec![Event::Text(dest.display().to_string())]);
        self.run(vec![enter_key(true)]);
        let ready = self.wait_for(hold, |h| h.find(Role::Button, "Exportar").is_some());
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
        assert!(harness.find(Role::Button, "Exportar").is_none(), "{button}");
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
    assert!(harness.find(Role::Button, "Exportar").is_some());
    assert!(is_empty_dir(out.path()), "Intro mantenido exportó");

    // A fresh Enter with no control focused is still the default action.
    harness.press(Key::Enter, Modifiers::NONE);
    let done = harness.wait_for(false, |h| h.has_name_containing("Abrir carpeta"));
    assert!(done && !is_empty_dir(&dest), "Intro sin foco no exportó");
}
