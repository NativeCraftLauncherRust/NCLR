// Terrain: vanilla's block and sky light as the ambient term, dimmed a little so the sun reads on
// top of it, the sun (or the moon) on every face turned toward it, and a cheap water look: a sky
// reflection that grows at grazing angles and a sharp highlight of the sun. The sun's cast shadow
// comes from the post pass, so a phone pays for no shadow-map pass at all.

override sun_light: f32 = 1.0;
override water: bool = true;

fn material(input: MaterialInput) -> Surface {
    let n = input.normal;
    let light = lite_ambient(input.lightmap) + lite_direct(n) * (1.15 * sun_light);
    var rgb = input.albedo.rgb * input.tint.rgb * light;

    // A torch, glowstone or lava shining through the bloom is what makes light read at night.
    rgb *= 1.0 + input.emission * 0.85;

    // Water: the surface faces up, so a grazing look mirrors the sky and catches the sun.
    if input.fluid == 1u && water {
        let view = normalize(input.position);
        let facing = clamp(dot(n, -view), 0.0, 1.0);
        let fresnel = pow(1.0 - facing, 4.0);
        let sky = frame.sky_color * mix(0.5, 1.0, lite_day());
        let half = normalize(frame.light_direction - view);
        let shine = pow(max(dot(n, half), 0.0), 96.0) * mix(0.1, 1.0, lite_day());
        rgb = mix(rgb, sky, fresnel * 0.5) + lite_sun_color() * (shine * 1.5);
    }

    return Surface(vec4<f32>(rgb, input.albedo.a * input.tint.a));
}
