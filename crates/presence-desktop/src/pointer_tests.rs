//! Drive real pointer events through the production controls, including focus changes.
use super::{model, profile_form, MainWindow, ProfileRow};
use presence_core::model::default_config;
use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        Key, Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent,
    },
    ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel, SharedString,
};
use std::{cell::RefCell, rc::Rc};

slint::slint! {
    import { UiButton, IconButton, Toggle, Choice, Input } from "../ui/components.slint";

    export component ControlsTest inherits Window {
        width: 400px;
        height: 300px;
        in-out property <int> button-clicks;
        in-out property <int> icon-clicks;
        in-out property <bool> checked;
        in-out property <int> choices;
        in-out property <int> choice-index;
        in-out property <string> input;
        UiButton { x: 10px; y: 10px; text: "Action"; clicked => { root.button-clicks += 1; } }
        IconButton { x: 200px; y: 10px; icon: "copy"; clicked => { root.icon-clicks += 1; } }
        Toggle { x: 10px; y: 80px; checked: root.checked; toggled => { root.checked = !root.checked; } }
        Choice { x: 10px; y: 120px; width: 180px; options: ["First", "Second"]; current-index <=> root.choice-index; selected(_) => { root.choices += 1; } }
        Input { x: 10px; y: 180px; width: 200px; text <=> root.input; }
        UiButton { x: 10px; y: 240px; text: "Disabled"; enabled: false; clicked => { root.button-clicks += 100; } }
    }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

fn draw(window: &MinimalSoftwareWindow) {
    let size = window.size();
    window.draw_if_needed(|renderer| {
        let mut pixels = vec![Rgb8Pixel::default(); size.width as usize * size.height as usize];
        renderer.render(&mut pixels, size.width as usize);
    });
}

fn click(window: &slint::Window, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn key(window: &slint::Window, text: SharedString) {
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

#[test]
fn appearance_selector_supports_clicks_arrow_keys_and_busy_state() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let ui = MainWindow::new().unwrap();
    window.set_size(PhysicalSize::new(1040, 720));
    ui.set_page(4);
    let selections = Rc::new(RefCell::new(Vec::new()));
    let recorded = selections.clone();
    let weak = ui.as_weak();
    ui.on_action(move |action, mode| {
        assert_eq!(action, "set-appearance");
        recorded.borrow_mut().push(mode.clone());
        weak.upgrade().unwrap().set_appearance(mode);
    });
    ui.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    draw(&window);

    click(ui.window(), 495.0, 288.0);
    assert_eq!(ui.get_appearance(), "light");
    click(ui.window(), 636.0, 288.0);
    assert_eq!(ui.get_appearance(), "system");
    key(ui.window(), Key::LeftArrow.into());
    assert_eq!(ui.get_appearance(), "light");
    key(ui.window(), Key::RightArrow.into());
    assert_eq!(ui.get_appearance(), "system");

    ui.set_busy(true);
    draw(&window);
    click(ui.window(), 350.0, 288.0);
    assert_eq!(
        selections.borrow().len(),
        4,
        "saving disables further changes"
    );
}

#[test]
fn profile_inputs_pause_while_backend_work_is_pending() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let ui = MainWindow::new().unwrap();
    ui.set_form(profile_form(&default_config().profiles[0]));
    let edits = Rc::new(RefCell::new(Vec::new()));
    let recorded = edits.clone();
    ui.on_field_edited(move |field, value| recorded.borrow_mut().push((field, value)));
    window.set_size(PhysicalSize::new(1470, 1000));
    ui.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    draw(&window);

    click(ui.window(), 350.0, 605.0);
    key(ui.window(), "X".into());
    assert_eq!(
        edits.borrow().len(),
        1,
        "profile field accepts normal edits"
    );
    assert_eq!(edits.borrow()[0].0, "name");
    let name = ui.get_form().name;

    ui.set_busy(true);
    draw(&window);
    key(ui.window(), "Y".into());
    click(ui.window(), 350.0, 605.0);
    key(ui.window(), "Y".into());
    assert_eq!(
        edits.borrow().len(),
        1,
        "busy fields cannot create ignored edits"
    );
    assert_eq!(ui.get_form().name, name);

    ui.set_busy(false);
    draw(&window);
    click(ui.window(), 350.0, 605.0);
    key(ui.window(), "Z".into());
    assert_eq!(
        edits.borrow().len(),
        2,
        "editing resumes after backend work"
    );
}

#[test]
fn first_click_activates_controls_and_transfers_keyboard_focus() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let controls = ControlsTest::new().unwrap();
    window.set_size(PhysicalSize::new(400, 300));
    controls.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    draw(&window);

    click(&window, 40.0, 30.0);
    assert_eq!(
        controls.get_button_clicks(),
        1,
        "first button click activates"
    );
    key(&window, Key::Space.into());
    assert_eq!(
        controls.get_button_clicks(),
        2,
        "click also gives keyboard focus"
    );
    click(&window, 216.0, 26.0);
    assert_eq!(controls.get_icon_clicks(), 1, "first icon click activates");
    key(&window, Key::Return.into());
    assert_eq!(
        controls.get_icon_clicks(),
        2,
        "Enter activates the focused icon"
    );
    click(&window, 27.0, 90.0);
    assert!(
        controls.get_checked(),
        "first toggle click changes its value"
    );
    key(&window, Key::Space.into());
    assert!(!controls.get_checked(), "toggle retains keyboard control");

    click(&window, 50.0, 140.0);
    draw(&window);
    click(&window, 50.0, 226.0);
    assert_eq!(
        controls.get_choices(),
        1,
        "first dropdown and option clicks select"
    );
    assert_eq!(controls.get_choice_index(), 1);
    click(&window, 30.0, 200.0);
    key(&window, "X".into());
    assert_eq!(controls.get_input(), "X", "first input click allows typing");
    click(&window, 40.0, 260.0);
    assert_eq!(
        controls.get_button_clicks(),
        2,
        "disabled buttons stay inactive"
    );
    controls.hide().unwrap();
    drop(controls);

    let ui = MainWindow::new().unwrap();
    let config = default_config();
    ui.set_form(profile_form(&config.profiles[0]));
    ui.set_profiles(model(
        config
            .profiles
            .iter()
            .map(|p| ProfileRow {
                id: p.id.clone().into(),
                title: p.name.clone().into(),
                selected: p.id == config.profiles[0].id,
                ..Default::default()
            })
            .collect(),
    ));
    ui.set_profile_names(model(
        config
            .profiles
            .iter()
            .map(|p| p.name.clone().into())
            .collect(),
    ));
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = actions.clone();
    ui.on_action(move |action, argument| recorded.borrow_mut().push((action, argument)));
    window.set_size(PhysicalSize::new(1470, 1000));
    ui.show().unwrap();
    draw(&window);

    for (y, page) in [(214.0, 1), (264.0, 2), (956.0, 4), (165.0, 0)] {
        click(&window, 100.0, y);
        assert_eq!(
            ui.get_page(),
            page,
            "first navigation click changes the page"
        );
        draw(&window);
    }
    click(&window, 800.0, 280.0);
    assert_eq!(
        actions.borrow().last().unwrap().1,
        "deep-work",
        "first card click selects the presence"
    );
    for (x, tab) in [(405.0, 1), (490.0, 2), (580.0, 3), (315.0, 0)] {
        click(&window, x, 460.0);
        assert_eq!(
            ui.get_editor_tab(),
            tab,
            "first editor-tab click switches tabs"
        );
        draw(&window);
    }
    key(&window, Key::RightArrow.into());
    assert_eq!(
        ui.get_editor_tab(),
        1,
        "clicked tabs retain arrow-key navigation"
    );
    ui.set_modal("delete-profile".into());
    draw(&window);
    click(&window, 100.0, 214.0);
    assert_eq!(ui.get_page(), 0, "dialogs still block background clicks");
    ui.hide().unwrap();
}

#[test]
fn priority_handles_drag_to_other_rows_and_leave_row_actions_interactive() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let ui = MainWindow::new().unwrap();
    ui.on_rule_drag_data(super::rule_drag_data);
    ui.on_rule_drag_id(super::rule_drag_id);
    ui.set_page(1);
    ui.set_rules(model(
        (0..3)
            .map(|i| super::RuleRow {
                id: format!("rule-{i}").into(),
                name: format!("Application {i}").into(),
                process: format!("app-{i}.exe").into(),
                profile: "My presence".into(),
                enabled: true,
            })
            .collect(),
    ));
    let drops = Rc::new(RefCell::new(Vec::new()));
    let recorded = drops.clone();
    ui.on_rule_reordered(move |source, target, after| {
        recorded.borrow_mut().push((source, target, after))
    });
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = actions.clone();
    ui.on_action(move |action, id| recorded.borrow_mut().push((action, id)));
    window.set_size(PhysicalSize::new(1470, 1000));
    ui.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    draw(&window);

    let drag = |from: (f32, f32), to: (f32, f32)| {
        let start = LogicalPosition::new(from.0, from.1);
        window.dispatch_event(WindowEvent::PointerMoved { position: start });
        window.dispatch_event(WindowEvent::PointerPressed {
            position: start,
            button: PointerEventButton::Left,
        });
        // Cross the DragArea threshold before moving over the destination.
        window.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(from.0 + 10.0, from.1),
        });
        window.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(to.0, to.1),
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(to.0, to.1),
            button: PointerEventButton::Left,
        });
        draw(&window);
    };
    drag((294.0, 438.0), (600.0, 620.0));
    assert_eq!(
        drops.borrow().as_slice(),
        &[("rule-0".into(), "rule-2".into(), true)],
        "first handle can cross several rows and drop after the last"
    );
    drag((294.0, 602.0), (600.0, 416.0));
    assert_eq!(
        drops.borrow().last().unwrap(),
        &("rule-2".into(), "rule-0".into(), false),
        "last handle can drop before the first"
    );
    let count = drops.borrow().len();
    drag((294.0, 438.0), (600.0, 160.0));
    drag((294.0, 438.0), (600.0, 450.0));
    drag((500.0, 438.0), (600.0, 620.0));
    click(&window, 294.0, 438.0);
    assert_eq!(
        drops.borrow().len(),
        count,
        "outside drops, same-row drops, dragging text, and clicks do not reorder"
    );
    key(&window, Key::DownArrow.into());
    assert_eq!(
        actions.borrow().last().unwrap(),
        &("rule-down".into(), "rule-0".into()),
        "focused handles retain keyboard priority control"
    );
    click(&window, 1358.0, 438.0);
    assert_eq!(
        actions.borrow().last().unwrap(),
        &("edit-rule".into(), "rule-0".into())
    );
    click(&window, 1311.0, 438.0);
    assert_eq!(
        actions.borrow().last().unwrap(),
        &("toggle-rule".into(), "rule-0".into())
    );
    ui.set_busy(true);
    drag((294.0, 438.0), (600.0, 620.0));
    assert_eq!(drops.borrow().len(), count, "saving disables drag and drop");
    ui.set_busy(false);
    ui.set_modal("rule".into());
    drag((294.0, 438.0), (600.0, 620.0));
    assert_eq!(
        drops.borrow().len(),
        count,
        "dialogs block background reordering"
    );
    // Plain text from another application cannot impersonate an internal rule.
    assert!(
        super::rule_drag_id(slint::DataTransfer::from(SharedString::from("rule-0"))).is_empty()
    );
    ui.hide().unwrap();
}
