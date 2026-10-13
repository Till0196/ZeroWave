#version 100
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
precision highp sampler2D;
// 8ビットへの丸めを自分でやるのは `highp` があるときだけ。理由は
// `TO_RGB` のコメントに書いた。
#define ROUND_EIGHT 1
#else
precision mediump float;
precision mediump sampler2D;
#endif
