#version 100
precision mediump float;
uniform sampler2D picture;
varying vec2 uv;
void main() { gl_FragColor = texture2D(picture, uv); }
