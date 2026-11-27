// The sky: a deeper blue overhead, a warm band around a low sun at sunrise and sunset, and a
// slight blue lift at night. The sun, moon, stars and clouds still draw over this.

fn sky(input: SkyInput) -> vec4<f32> {
    let d = input.direction;
    let day = lite_day();
    let low = lite_sun_low();
    let up = clamp(d.y, 0.0, 1.0);
    let toward = max(dot(d, frame.sun_direction), 0.0);

    // Darken and cool the zenith, written in gamma and decoded: keeps the horizon light.
    let deep = smoothstep(0.04, 0.9, up);
    let tint = mix(vec3<f32>(1.0), vec3<f32>(0.74, 0.86, 1.0), deep);
    var base = input.color.rgb * pow(tint, vec3<f32>(2.2));

    // The warm band: strongest on the horizon under the sun, a little all around it.
    let band = (1.0 - smoothstep(0.0, 0.42, up)) * (0.35 + 0.65 * pow(toward, 2.0));
    let warm = vec3<f32>(1.0, 0.36, 0.12) * (band * low * day * 0.55);

    // Night: a faint blue above, so the sky is not flat black.
    let night = (1.0 - day) * up * 0.06;
    base += vec3<f32>(0.18, 0.32, 0.85) * night;

    // Rain washes both out.
    let wet = 1.0 - frame.rain * 0.45;
    return vec4<f32>((base + warm) * wet, input.color.a);
}
