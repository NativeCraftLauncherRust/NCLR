//! 3D Backpack: the player's inventory as a turning 3D showcase.
//!
//! Opening the inventory (the key, or the launcher's touch button) opens this
//! mod's own screen instead: the player stands in a 3D stage on the left,
//! turning with a drag, and on the right the backpack's stacks sit in a grid.
//! Picking a stack shows it in 3D above the grid: a block item turns as a
//! real block, anything else as its icon, larger. A button opens the vanilla
//! inventory for the usual clicks.
use mod_api::{
    ClientHost, ClientTickPreEvent, DrawContext, Flow, GuiEntity, GuiEntityLook, GuiEntityView,
    Hooks, ItemStack, Scene, SceneBlock, SceneView, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent, ScreenMouseDraggedPreEvent,
    ScreenOpening, ScreenOpeningEvent, ScreenRenderPostEvent,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The mod's id: the name of the screen it opens.
const ID: &str = "threed_backpack";

/// What a drag is turning.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    None,
    Player,
    Item,
}

/// What the screen remembers between frames.
struct State {
    /// Body yaw of the player model, degrees.
    player_yaw: f32,
    /// Tilt of the player model, degrees.
    player_pitch: f32,
    /// Yaw of the shown item, degrees.
    item_yaw: f32,
    /// The stack picked in the grid; none shows the first filled one.
    picked: Option<usize>,
    /// The stack the pointer is over.
    hovering: Option<usize>,
    /// What the pointer is turning.
    turn: Turn,
}

impl Default for State {
    fn default() -> Self {
        Self {
            player_yaw: 0.0,
            player_pitch: 0.0,
            item_yaw: 30.0,
            picked: None,
            hovering: None,
            turn: Turn::None,
        }
    }
}

#[derive(Default)]
pub struct Backpack {
    state: Mutex<State>,
    /// The next tick opens the screen: the hook that hears the inventory open
    /// only reads the host.
    pending_open: AtomicBool,
    /// The next tick closes the screen.
    pending_close: AtomicBool,
    /// The next tick opens the vanilla inventory.
    pending_vanilla: AtomicBool,
    /// The next inventory open is the player's own: let it through.
    allow_vanilla: AtomicBool,
}

/// The screen's geometry, computed once a frame.
struct Layout {
    panel: [f32; 4],
    stage: [f32; 4],
    grid: [f32; 4],
    showcase: [f32; 4],
    vanilla: [f32; 4],
    close: [f32; 4],
    columns: usize,
    slot: f32,
}

fn layout(width: f32, height: f32) -> Layout {
    let pw = 306.0_f32.min(width - 12.0);
    let ph = 268.0_f32.min(height - 12.0);
    let x = ((width - pw) / 2.0).floor();
    let y = ((height - ph) / 2.0).floor();
    let left = x + 8.0;
    let stage_w = (pw * 0.44).floor();
    let bottom = y + ph - 8.0;
    let slot = 18.0;
    let columns = (((pw - stage_w - 24.0) / slot).floor() as usize).clamp(4, 9);
    let grid_x = left + stage_w + 8.0;
    Layout {
        panel: [x, y, pw, ph],
        stage: [left, y + 26.0, left + stage_w, bottom - 30.0],
        grid: [
            grid_x,
            y + 30.0,
            grid_x + columns as f32 * slot,
            y + 30.0 + 4.0 * slot,
        ],
        showcase: [grid_x, y + 30.0 + 4.0 * slot + 6.0, x + pw - 8.0, bottom - 26.0],
        vanilla: [left, bottom - 20.0, left + 74.0, bottom],
        close: [x + pw - 8.0 - 54.0, bottom - 20.0, x + pw - 8.0, bottom],
        columns,
        slot,
    }
}

fn inside(rect: [f32; 4], at: [f32; 2]) -> bool {
    at[0] >= rect[0] && at[0] < rect[2] && at[1] >= rect[1] && at[1] < rect[3]
}

/// The GUI size: the screen hooks' `ScreenView` does not carry it, so the last
/// frame's size is kept.
static SCREEN_SIZE: Mutex<[f32; 2]> = Mutex::new([320.0, 240.0]);

/// The pointer in GUI pixels, as the last frame or input heard it.
static POINTER: Mutex<[f32; 2]> = Mutex::new([0.0, 0.0]);

fn pointer() -> [f32; 2] {
    *POINTER.lock().unwrap()
}

impl Backpack {
    /// The stacks of the player's inventory, in menu order.
    fn stacks(&self, host: &dyn ClientHost) -> Vec<ItemStack> {
        host.local_player()
            .and_then(|player| player.inventory(host))
            .unwrap_or_default()
    }

    /// The box of slot `index` in the grid.
    fn slot_rect(&self, layout: &Layout, index: usize) -> [f32; 4] {
        let (col, row) = (index % layout.columns, index / layout.columns);
        let x = layout.grid[0] + col as f32 * layout.slot;
        let y = layout.grid[1] + row as f32 * layout.slot;
        [x, y, x + layout.slot, y + layout.slot]
    }

    fn draw(&self, host: &dyn ClientHost, draw: &mut dyn DrawContext) {
        let [w, h] = draw.size();
        let l = layout(w, h);
        let stacks = self.stacks(host);
        let mut state = self.state.lock().unwrap();

        // The world behind the panel.
        draw.fill([0.0, 0.0, w, h], 0xC80A0A10);
        // The panel, with a lighter edge.
        draw.fill(l.panel, 0xF21A1A22);
        let [x, y, pw, ph] = l.panel;
        draw.fill([x, y, pw, 1.0], 0xFF3C3C4C);
        draw.fill([x, y + ph - 1.0, pw, 1.0], 0xFF3C3C4C);
        draw.fill([x, y, 1.0, ph], 0xFF3C3C4C);
        draw.fill([x + pw - 1.0, y, 1.0, ph], 0xFF3C3C4C);
        draw.text("3D 背包", [x + 9.0, y + 8.0], 0xFFFFFFFF, false);
        let hint = "拖动角色或物品旋转";
        let tw = draw.text_width(hint);
        draw.text(hint, [x + pw - tw - 9.0, y + 9.0], 0xFF8C8CA0, false);

        // The stage: a soft floor, then the player standing on it.
        let stage = l.stage;
        draw.fill(stage, 0xFF101018);
        let (cx, cy) = (
            (stage[0] + stage[2]) / 2.0,
            stage[1] + (stage[3] - stage[1]) * 0.82,
        );
        for i in 0..6 {
            let t = (i as f32 + 0.5) / 6.0;
            let half = 34.0 * (1.0 - t * t).max(0.0).sqrt();
            let alpha = (56.0 * (1.0 - t)) as u32;
            draw.fill(
                [cx - half, cy - 6.0 + t * 12.0, half * 2.0, 2.4],
                alpha << 24,
            );
        }
        let size = (stage[3] - stage[1]) / 4.4;
        let view = GuiEntityView::new(
            stage,
            size.max(18.0),
            0.0,
            GuiEntityLook::Turned {
                yaw: state.player_yaw,
                pitch: 0.0,
            },
        );
        let _ = draw.entity(&GuiEntity::kind("minecraft:player"), view);

        // The grid of stacks.
        state.hovering = None;
        let mouse = pointer();
        for (i, stack) in stacks.iter().enumerate() {
            if i / l.columns >= 4 {
                break;
            }
            let cell = self.slot_rect(&l, i);
            draw.fill(
                [cell[0] + 1.0, cell[1] + 1.0, l.slot - 2.0, l.slot - 2.0],
                0xFF25252E,
            );
            let hovered = inside(cell, mouse);
            if hovered && stack.count() > 0 {
                state.hovering = Some(i);
            }
            if stack.count() > 0 {
                let _ = draw.item_stack(stack, [cell[0] + 1.0, cell[1] + 1.0], l.slot - 2.0);
                if stack.count() > 1 {
                    let label = stack.count().to_string();
                    let tw = draw.text_width(&label);
                    draw.text(&label, [cell[2] - tw - 1.0, cell[3] - 9.0], 0xFFFFFFFF, true);
                }
            }
            if state.picked == Some(i) {
                draw.fill([cell[0], cell[1], l.slot, 1.0], 0xFF7FD4FF);
                draw.fill([cell[0], cell[3] - 1.0, l.slot, 1.0], 0xFF7FD4FF);
                draw.fill([cell[0], cell[1], 1.0, l.slot], 0xFF7FD4FF);
                draw.fill([cell[2] - 1.0, cell[1], 1.0, l.slot], 0xFF7FD4FF);
            } else if hovered {
                draw.fill(
                    [cell[0] + 1.0, cell[1] + 1.0, l.slot - 2.0, l.slot - 2.0],
                    0x30FFFFFF,
                );
            }
        }

        // The showcase: the shown stack as a turning 3D block, or larger as an icon.
        let showcase = l.showcase;
        draw.fill(showcase, 0xFF101018);
        let shown = state
            .picked
            .filter(|&i| stacks.get(i).is_some_and(|s| s.count() > 0))
            .or_else(|| stacks.iter().position(|s| s.count() > 0));
        if let Some(index) = shown
            && let Some(stack) = stacks.get(index)
        {
            let id = stack.item().id(host);
            let (w2, h2) = (
                (showcase[2] - showcase[0]).max(1.0),
                (showcase[3] - showcase[1]).max(1.0),
            );
            let cx2 = (showcase[0] + showcase[2]) / 2.0;
            let cy2 = (showcase[1] + showcase[3]) / 2.0;
            match host.block_state_id(&id) {
                Some(state_id) => {
                    let scene = Scene::new(vec![SceneBlock::new([0, 0, 0], state_id, None)]);
                    let view = SceneView::new(
                        [0.5, 0.5, 0.5],
                        state.item_yaw,
                        22.0,
                        (w2.min(h2) / 2.2).max(8.0),
                    );
                    let _ = draw.draw_scene(
                        [
                            showcase[0] + 4.0,
                            showcase[1] + 4.0,
                            showcase[2] - 4.0,
                            showcase[3] - 4.0,
                        ],
                        &scene,
                        view,
                    );
                }
                None => {
                    let side = (w2.min(h2) * 0.75).max(16.0);
                    let _ = draw.item_stack(stack, [cx2 - side / 2.0, cy2 - side / 2.0], side);
                }
            }
            let name = id.trim_start_matches("minecraft:").to_owned();
            let tw = draw.text_width(&name);
            draw.text(
                &name,
                [cx2 - tw / 2.0, showcase[3] - 13.0],
                0xFFB9B9CC,
                false,
            );
        }

        // The two buttons.
        draw.fill(
            [l.vanilla[0], l.vanilla[1], l.vanilla[2] - l.vanilla[0], 16.0],
            0xFF2F2F3C,
        );
        draw.text("原版背包", [l.vanilla[0] + 6.0, l.vanilla[1] + 4.0], 0xFFCFCFE0, false);
        draw.fill(
            [l.close[0], l.close[1], l.close[2] - l.close[0], 16.0],
            0xFF2F2F3C,
        );
        draw.text("关闭", [l.close[0] + 6.0, l.close[1] + 4.0], 0xFFCFCFE0, false);
    }

    /// A press at `at`: pick a stack, start a turn, or press a button.
    fn press(&self, host: &dyn ClientHost, at: [f32; 2]) {
        let [w, h] = *SCREEN_SIZE.lock().unwrap();
        let l = layout(w, h);
        if inside(l.vanilla, at) {
            self.pending_vanilla.store(true, Ordering::Relaxed);
            self.pending_close.store(true, Ordering::Relaxed);
            return;
        }
        if inside(l.close, at) {
            self.pending_close.store(true, Ordering::Relaxed);
            return;
        }
        let stacks = self.stacks(host);
        let mut state = self.state.lock().unwrap();
        if inside(l.stage, at) {
            state.turn = Turn::Player;
            return;
        }
        if inside(l.showcase, at) {
            state.turn = Turn::Item;
            return;
        }
        if inside(l.grid, at) {
            let col = ((at[0] - l.grid[0]) / l.slot).floor() as usize;
            let row = ((at[1] - l.grid[1]) / l.slot).floor() as usize;
            let index = row * l.columns + col;
            if let Some(stack) = stacks.get(index)
                && stack.count() > 0
            {
                state.picked = Some(index);
            }
        }
    }

    fn drag(&self, delta: [f32; 2]) {
        let mut state = self.state.lock().unwrap();
        match state.turn {
            Turn::Player => {
                state.player_yaw = (state.player_yaw + delta[0] * 1.2).rem_euclid(360.0);
                state.player_pitch = (state.player_pitch - delta[1] * 0.6).clamp(-40.0, 40.0);
            }
            Turn::Item => {
                state.item_yaw = (state.item_yaw + delta[0] * 1.6).rem_euclid(360.0);
            }
            Turn::None => {}
        }
    }
}

#[mod_api::horizon_mod]
impl Hooks for Backpack {
    /// The inventory is about to open the vanilla screen: keep it shut and open
    /// ours on the next tick instead.
    fn screen_opening(
        &self,
        _host: &dyn ClientHost,
        event: &mut ScreenOpeningEvent<'_>,
    ) -> Option<ScreenOpening> {
        if self.allow_vanilla.swap(false, Ordering::Relaxed) {
            return None;
        }
        if event.new.id != "inventory" || event.new.menu.is_some() {
            return None;
        }
        self.pending_open.store(true, Ordering::Relaxed);
        Some(ScreenOpening::Cancel)
    }

    fn client_tick_pre(&self, host: &mut dyn ClientHost, _: &mut ClientTickPreEvent) {
        if self.pending_vanilla.swap(false, Ordering::Relaxed) {
            self.allow_vanilla.store(true, Ordering::Relaxed);
            host.open_inventory();
        } else if self.pending_close.swap(false, Ordering::Relaxed) {
            host.mod_screen_close(ID);
        } else if self.pending_open.swap(false, Ordering::Relaxed) {
            host.mod_screen_open(ID);
        }
    }

    fn screen_closing(&self, host: &dyn ClientHost, _: &mut ScreenClosingEvent<'_>) {
        if host.mod_screen() == Some(ID) {
            self.state.lock().unwrap().turn = Turn::None;
        }
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        if host.mod_screen() != Some(ID) {
            return;
        }
        let [w, h] = event.draw.size();
        *SCREEN_SIZE.lock().unwrap() = [w, h];
        *POINTER.lock().unwrap() = event.screen.mouse;
        self.draw(host, &mut *event.draw);
    }

    fn screen_mouse_button_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        *POINTER.lock().unwrap() = event.screen.mouse;
        self.press(host, event.screen.mouse);
        Flow::Cancel
    }

    fn screen_mouse_button_released_pre(
        &self,
        host: &dyn ClientHost,
        _: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        self.state.lock().unwrap().turn = Turn::None;
        Flow::Cancel
    }

    fn screen_mouse_dragged_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseDraggedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        *POINTER.lock().unwrap() = event.screen.mouse;
        self.drag(event.delta);
        Flow::Cancel
    }

    /// Escape closes the screen, as it closes a vanilla one.
    fn screen_key_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        if event.key == "key.keyboard.escape" {
            self.pending_close.store(true, Ordering::Relaxed);
            return Flow::Cancel;
        }
        Flow::Continue
    }
}
