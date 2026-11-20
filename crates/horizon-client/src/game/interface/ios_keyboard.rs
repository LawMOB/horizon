//! Native keyboard state and the space left above it for chat.
use super::{Game, Screen};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SoftKeyboard {
    pub(super) away: bool,
    pub(super) row: Option<[i32; 4]>,
}

impl Game<'_> {
    /// iOS's native keyboard owns the title's text fields; other platforms keep
    /// the existing keyboard/controller focus rules.
    pub(super) fn keyboard_title_input_active(&self) -> bool {
        #[cfg(target_os = "ios")]
        {
            super::control::title_owns_text(&self.screen, self.title.is_some())
        }
        #[cfg(not(target_os = "ios"))]
        self.title_input_active()
    }

    /// Chat uses the part above the iPhone keyboard for layout, drawing and hit
    /// tests. Convert UIKit's points through the window scale and the current
    /// GUI scale, keeping the result in the integer GUI pixels the build uses.
    pub(super) fn chat_height(&self, height: f32, scale: f32) -> f32 {
        let points = horizon_client::screen_keyboard::height_points();
        if self.screen != Screen::Chat || points <= 0.0 || scale <= 0.0 {
            return height;
        }
        let factor = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        (height - (points * factor) as f32 / scale).floor().max(1.0)
    }
}
