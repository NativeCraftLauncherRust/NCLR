// Shared by every stage: only the frame is in scope here, so a stage's own resources stay out.
// The sky and the sun are given in linear light, and the warm constants below were chosen in
// gamma, so they are decoded with a 2.2 power where they tint light.

fn lite_luminance(c: vec3<f32>) -> f32 {
    return dot(c, frame.luminance);
}

// 1 while the sun is up in the Overworld, easing across the horizon; 0 elsewhere and at night.
fn lite_day() -> f32 {
    if frame.dimension != 0 {
        return 0.0;
    }
    return smoothstep(-0.06, 0.12, frame.sun_direction.y);
}

// 1 with the sun on the horizon, 0 once it is high: the sunrise and sunset window.
fn lite_sun_low() -> f32 {
    return 1.0 - smoothstep(0.0, 0.45, abs(frame.sun_direction.y));
}

// The colour of the direct light: white at noon, gold at sunrise and sunset, moonlight at night.
fn lite_sun_color() -> vec3<f32> {
    let noon = vec3<f32>(1.0, 0.975, 0.93);
    let gold = vec3<f32>(1.0, 0.62, 0.34);
    let warm = mix(noon, gold, lite_sun_low());
    let moon = vec3<f32>(0.55, 0.68, 1.0);
    return mix(moon, warm, lite_day());
}
