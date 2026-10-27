#version 300 es
#extension GL_EXT_YUV_target : require
precision highp float;
precision highp int;
uniform highp __samplerExternal2DY2YEXT source;
uniform mat4 textureTransform;
uniform int maximum;
out vec4 color;
vec3 raw(vec2 pixel) {
    vec2 coordinate = pixel / 128.0;
    coordinate.y = 1.0 - coordinate.y;
    return texture(source, (textureTransform * vec4(coordinate, 0.0, 1.0)).xy).xyz * float(maximum);
}
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    int y = (p.x + p.y * 13) & maximum;
    int u = ((p.x / 2) * 7 + (p.y / 2) * 19 + 17) & maximum;
    float actualY = raw(vec2(p) + 0.5).x;
    vec2 actualUV = raw(vec2((p / 2) * 2 + 1)).yz;
    bool exact = abs(actualY - float(y)) < 0.6 &&
                 all(lessThan(abs(actualUV - vec2(u, maximum - u)), vec2(0.6)));
    color = exact ? vec4(1.0) : vec4(0.0, 0.0, 0.0, 1.0);
}
