#version 300 es
#extension GL_EXT_YUV_target : require
precision highp float;
precision highp int;
uniform highp __samplerExternal2DY2YEXT source;
uniform mat4 textureTransform;
uniform ivec4 picture; // logical width/height, padded width/height
uniform ivec4 colorimetry; // ten bit, full range, matrix, HDR
uniform int packingVersion;
in vec2 uv;
out vec4 color;

vec3 raw(vec2 pixel) {
    vec2 coordinate = pixel / vec2(picture.z, picture.w * 2);
    coordinate.y = 1.0 - coordinate.y;
    return texture(source, (textureTransform * vec4(coordinate, 0.0, 1.0)).xy).xyz;
}

vec3 reconstruct(ivec2 pixel) {
    pixel = clamp(pixel, ivec2(0), picture.xy - 1);
    float maximum = colorimetry.x != 0 ? 1023.0 : 255.0;
    float y = raw(vec2(pixel) + 0.5).x * maximum;
    vec2 chroma;
    if (packingVersion == 2) {
        vec2 base = raw(vec2((pixel / 2) * 2 + 1)).yz * maximum;
        if ((pixel.x & 1) != 0) {
            chroma = vec2(raw(vec2(pixel.x / 2, picture.w + pixel.y) + 0.5).x,
                          raw(vec2(picture.z / 2 + pixel.x / 2, picture.w + pixel.y) + 0.5).x) * maximum;
        } else if ((pixel.y & 1) != 0) {
            chroma = raw(vec2(pixel.x + 1, picture.w + (pixel.y / 2) * 2 + 1)).yz * maximum;
        } else {
            chroma = vec2(colorimetry.x != 0 ? 512.0 : 128.0);
        }
        chroma = clamp(base + 2.0 * (chroma - (colorimetry.x != 0 ? 512.0 : 128.0)), 0.0, maximum);
    } else if ((pixel.x & 1) == 0) {
        chroma = raw(vec2(pixel.x + 1, pixel.y * 2 + 1)).yz * maximum;
    } else {
        chroma = vec2(raw(vec2(pixel.x / 2, picture.w + pixel.y) + 0.5).x,
                      raw(vec2(picture.z / 2 + pixel.x / 2, picture.w + pixel.y) + 0.5).x) * maximum;
    }
    float scale = colorimetry.x != 0 ? 4.0 : 1.0;
    if (colorimetry.y != 0) {
        y /= maximum;
        chroma = (chroma - 128.0 * scale) / maximum;
    } else {
        y = (y - 16.0 * scale) / (219.0 * scale);
        chroma = (chroma - 128.0 * scale) / (224.0 * scale);
    }
    vec2 krkb = colorimetry.z == 2 ? vec2(0.2627, 0.0593) :
                colorimetry.z == 0 ? vec2(0.299, 0.114) : vec2(0.2126, 0.0722);
    float r = y + 2.0 * (1.0 - krkb.x) * chroma.y;
    float b = y + 2.0 * (1.0 - krkb.y) * chroma.x;
    float g = (y - krkb.x * r - krkb.y * b) / (1.0 - krkb.x - krkb.y);
    vec3 rgb = max(vec3(r, g, b), 0.0);
    if (colorimetry.x == 0) return rgb;
    if (colorimetry.w != 0) {
        vec3 p = pow(clamp(rgb, 0.0, 1.0), vec3(1.0 / 78.84375));
        return 10000.0 * pow(max(p - 0.8359375, 0.0) / max(18.8515625 - 18.6875 * p, 0.000001), vec3(1.0 / 0.1593017578125));
    }
    rgb = mix(rgb / 12.92, pow((rgb + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), rgb));
    return 80.0 * mat3(0.627404, 0.069097, 0.016391, 0.329283, 0.919540, 0.088013, 0.043313, 0.011362, 0.895595) * rgb;
}

void main() {
    vec2 position = vec2(uv.x, 1.0 - uv.y) * vec2(picture.xy) - 0.5;
    ivec2 base = ivec2(floor(position));
    vec2 weight = fract(position);
    // Interpolate reconstructed color, never the adjacent packed sample regions.
    vec3 rgb = mix(mix(reconstruct(base), reconstruct(base + ivec2(1, 0)), weight.x),
                   mix(reconstruct(base + ivec2(0, 1)), reconstruct(base + ivec2(1, 1)), weight.x), weight.y);
    if (colorimetry.x != 0) {
        vec3 p = pow(max(rgb / 10000.0, 0.0), vec3(0.1593017578125));
        rgb = pow((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p), vec3(78.84375));
    }
    color = vec4(rgb, 1.0);
}
