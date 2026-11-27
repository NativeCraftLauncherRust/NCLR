// The last pass: takes the sun's shadow back in, adds the bloom, rolls the brightest light off
// instead of clipping it, then a gentle S-curve, saturation and a light vignette. Written for
// display values where the look wants them, so the grade runs on the encoded picture and the
// light stays linear.

override glow: f32 = 0.45;
override saturation: f32 = 1.08;
override vignette: bool = true;
override shadows: bool = true;

fn post(input: PostInput) -> vec4<f32> {
    var c = input.color.rgb;
    c += textureSampleLevel(bloom, linear_sampler, input.uv, 0.0).rgb * glow;

    if shadows && frame.dimension == 0 && input.depth > 0.0 {
        // The mask is a quarter-size target: four taps smooth its blocks into a soft edge.
        let texel = 1.0 / (frame.resolution * 0.5);
        let vis = 0.25 * (textureSampleLevel(sun, linear_sampler, input.uv + vec2<f32>(-texel.x, 0.0), 0.0).r
            + textureSampleLevel(sun, linear_sampler, input.uv + vec2<f32>(texel.x, 0.0), 0.0).r
            + textureSampleLevel(sun, linear_sampler, input.uv + vec2<f32>(0.0, -texel.y), 0.0).r
            + textureSampleLevel(sun, linear_sampler, input.uv + vec2<f32>(0.0, texel.y), 0.0).r);
        // Clouds and anything else high overhead keep the sun; only the ground and what stands on
        // it is shaded.
        let high = view_position(input.uv, input.depth).y + frame.camera_position.y > 160.0;
        if !high {
            // How much of the light the sun is responsible for: most of it at noon, little when
            // it hangs low. The sky's own light stays on a shadowed surface.
            let share = mix(0.28, 0.60, smoothstep(0.1, 0.6, frame.sun_direction.y));
            c *= 1.0 - (1.0 - vis) * share;
        }
    }

    // Soft shoulder: the sun's highlights keep their colour instead of turning white.
    c = c / (1.0 + 0.10 * c);

    var g = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    g = mix(vec3<f32>(lite_luminance(g)), g, saturation);
    g = mix(g, smoothstep(vec3<f32>(0.0), vec3<f32>(1.0), g), 0.10);
    c = pow(max(g, vec3<f32>(0.0)), vec3<f32>(2.2));

    if vignette {
        let aspect = frame.resolution.x / frame.resolution.y;
        let d = length((input.uv - 0.5) * vec2<f32>(aspect, 1.0)) / length(vec2<f32>(aspect, 1.0) * 0.5);
        c *= pow(1.0 - smoothstep(0.6, 1.15, d) * 0.22, 2.2);
    }
    return vec4<f32>(c, 1.0);
}
