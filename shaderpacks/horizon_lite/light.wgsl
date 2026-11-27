// The light on a surface, shared by [material] and [entity]. The sun's cast shadow is not made
// here: the post pass draws it from the screen's depth (see post/composite.wgsl), which keeps a
// second terrain pass off a phone's GPU.

// The direct light on a surface: how squarely it faces the sun (or the moon), times the light's
// colour, and a tenth of it at night. 0 outside the Overworld.
fn lite_direct(normal: vec3<f32>) -> vec3<f32> {
    if frame.dimension != 0 {
        return vec3<f32>(0.0);
    }
    let ndl = max(dot(normal, frame.light_direction), 0.0);
    let body = mix(0.14, 1.0, lite_day());
    return lite_sun_color() * (ndl * body);
}

// How much of vanilla's own light stays: less in the Overworld, where the sun adds on top of it,
// and nearly all of it in the Nether and the End, which have no sun.
fn lite_ambient(lightmap: vec3<f32>) -> vec3<f32> {
    return lightmap * select(0.85, 0.6, frame.dimension == 0);
}
