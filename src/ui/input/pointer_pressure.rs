use eframe::egui;

pub fn current_pointer_pressure(ctx: &egui::Context, pointer: egui::Pos2) -> f32 {
    const MAX_TOUCH_DISTANCE_POINTS: f32 = 12.0;
    ctx.input(|input| {
        input
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                egui::Event::Touch { pos, force, .. }
                    if pos.distance(pointer) <= MAX_TOUCH_DISTANCE_POINTS =>
                {
                    Some(force.unwrap_or(1.0))
                }
                _ => None,
            })
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    })
}
