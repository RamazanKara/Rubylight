#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
precision highp float;
uniform highp samplerExternalOES source;
uniform mat4 textureTransform;
in vec2 uv;
out vec4 color;
void main() {
    color = vec4(texture(source, (textureTransform * vec4(uv, 0.0, 1.0)).xy).rgb, 1.0);
}
