// Half-resolution bloom: the brightest things (the sun, lava, torches at night) bleed a little
// light around them. Eight bilinear taps on two rings, so it is cheap on a phone GPU.

const THRESHOLD: f32 = 0.60;
const SOFT: f32 = 0.30;

fn lite_bright(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(previous, linear_sampler, uv, 0.0).rgb;
    // The sky is bright by day, but it should not bloom like a lamp does.
    let sky = select(1.0, 0.4, depth_at(uv) == 0.0);
    return c * (smoothstep(THRESHOLD, THRESHOLD + SOFT, lite_luminance(c)) * sky);
}

fn post(input: PostInput) -> vec4<f32> {
    // This pass draws at half the world's size, so one of its texels is two of the world's.
    let texel = 2.0 / frame.resolution;
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let angle = f32(i) * 1.5707963 + 0.4;
        let ring = vec2<f32>(cos(angle), sin(angle)) * texel;
        sum += lite_bright(input.uv + ring * 2.0);
        sum += lite_bright(input.uv + ring * 5.0) * 0.7;
    }
    return vec4<f32>(sum / 6.8, 1.0);
}
