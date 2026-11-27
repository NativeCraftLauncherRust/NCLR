// Entities and block entities, lit as terrain is, so a mob in the sun is as bright as the ground
// it stands on. Vanilla's hurt and TNT flash is kept, and a glowing layer (eyes, the warden) is
// drawn at full brightness.

override sun_light: f32 = 1.0;

fn entity(input: EntityInput) -> Surface {
    let n = input.normal;
    var base = input.albedo.rgb * input.tint.rgb * input.shade;

    // Vanilla lays the flash over the surface in gamma, before the light.
    let gamma = vec3<f32>(1.0 / 2.2);
    base = pow(mix(pow(base, gamma), pow(input.overlay.rgb, gamma), input.overlay.a), vec3<f32>(2.2));

    var light = lite_ambient(input.lightmap) + lite_direct(n) * (1.15 * sun_light);
    // A layer vanilla draws at full brightness (spider and enderman eyes, the warden's glow).
    if input.emission > 0.0 {
        light = input.lightmap;
    }
    return Surface(vec4<f32>(base * light, input.albedo.a * input.tint.a));
}
