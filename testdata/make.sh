#!/bin/sh
# 試験の素材を作る。映像は testsrc・testsrc2、音は aevalsrc の正弦波で、どれも ffmpeg が
# その場で作った物。ffmpeg は libx264・libx265・mpeg2video・aac 入り（FFMPEG で名指しもできる）。
#     sh testdata/make.sh
# 作った物は git に入れない。
# エンコーダーのバージョンで中身のバイトは変わる。試験が見るのは寸法・枚数・構造なので、それで構わない。
set -eu
cd "$(dirname "$0")"

ff() { "${FFMPEG:-ffmpeg}" -hide_banner -loglevel error -y "$@"; }
same="-fflags +bitexact -flags:a +bitexact -map_metadata -1"
ts="-f mpegts -mpegts_start_pid 0x100 -muxrate 0"
stereo() { echo "aevalsrc=0.3*sin(2*PI*$1*t)|0.3*sin(2*PI*$2*t):s=48000:d=$3"; }

# H.264 のベースライン（CAVLC）。180 行は 16 で割れないので、下を切り落とす寸法の確かめにもなる。音も付ける。
ff -f lavfi -i testsrc2=s=320x180:r=30000/1001:d=1 -f lavfi -i "$(stereo 440 660 1)" \
    -c:v libx264 -profile:v baseline -preset medium -g 15 -pix_fmt yuv420p -flags +bitexact \
    -x264-params threads=1 \
    -c:a aac -b:a 96k $same $ts h264-baseline.ts

# H.264 のハイ（CABAC、B ピクチャの階層、8x8 の変換、重み付きの予測）。
ff -f lavfi -i testsrc2=s=352x288:r=30000/1001:d=1 \
    -c:v libx264 -profile:v high -preset medium -g 30 -bf 3 -pix_fmt yuv420p -flags +bitexact \
    -x264-params threads=1:b-pyramid=normal:weightp=2:8x8dct=1 \
    $same $ts h264-high.ts

# HEVC の Main 10。オープン GOP の二つ目の CRA から切り出すので、先頭に捨てるリーディングピクチャーがある。
ff -f lavfi -i testsrc2=s=416x240:r=30000/1001:d=1.2 \
    -c:v libx265 -pix_fmt yuv420p10le -preset fast \
    -x265-params log-level=error:frame-threads=1:pools=none:keyint=12:min-keyint=12:open-gop=1:bframes=3:scenecut=0:repeat-headers=1 \
    $same $ts hevc-whole.ts
ff -ss 0.38 -i hevc-whole.ts -c copy $same $ts hevc-main10.ts
rm hevc-whole.ts

# MPEG-2 のインターレース（トップが先のフレームピクチャ、フィールドの DCT と予測）。毎秒 60 枚の
# 動く映像から一枚おきにフィールドを取って組み合わせるので、動く所は櫛状になる。音も付ける。
ff -f lavfi -i testsrc=s=352x240:r=60000/1001:d=1.2,scroll=h=0.01 -f lavfi -i "$(stereo 440 660 1.2)" \
    -vf tinterlace=mode=interleave_top,setfield=tff \
    -c:v mpeg2video -q:v 10 -g 24 -bf 2 -flags +ilme+ildct+bitexact -top 1 \
    -c:a aac -b:a 96k $same $ts mpeg2-interlaced.ts

# 毎秒 24 フレームの映像を 2:3 でテレシネして MPEG-2 のインターレースで符号化した物。
ff -f lavfi -i testsrc=s=192x128:r=24000/1001:d=3.4,scroll=h=0.01 \
    -vf telecine=first_field=top:pattern=23,setfield=tff \
    -c:v mpeg2video -q:v 6 -g 15 -bf 2 -flags +ilme+ildct+bitexact -top 1 \
    $same $ts mpeg2-telecine.ts

# テレシネを ffmpeg の telecine フィルターの出す物と突き合わせる（輝度だけの Y4M）。
ff -f lavfi -i testsrc=s=64x48:r=24000/1001:d=1.34,scroll=h=0.03,format=gray \
    -f yuv4mpegpipe -strict -1 telecine-source.y4m
ff -f lavfi -i testsrc=s=64x48:r=24000/1001:d=1.34,scroll=h=0.03,format=gray \
    -vf telecine=first_field=top:pattern=23 \
    -f yuv4mpegpipe -strict -1 telecine-23.y4m

# 番組が二つ（それぞれ MPEG-2 と AAC）。途中で番組を切り替える試験に。
ff -f lavfi -i testsrc2=s=160x96:r=30000/1001:d=2 -f lavfi -i "$(stereo 440 660 2)" \
    -f lavfi -i testsrc=s=160x96:r=30000/1001:d=2 -f lavfi -i "$(stereo 550 770 2)" \
    -map 0:v -map 1:a -map 2:v -map 3:a \
    -c:v mpeg2video -q:v 10 -g 6 -bf 0 -flags +bitexact -c:a aac -b:a 64k \
    -program program_num=1:st=0:st=1 -program program_num=2:st=2:st=3 \
    $same $ts two-programmes.ts

# 音が三本の番組（MPEG-2 と AAC 三本）。音を切り替える試験に。
ff -f lavfi -i testsrc2=s=160x96:r=30000/1001:d=2 -f lavfi -i "$(stereo 440 660 2)" \
    -f lavfi -i "$(stereo 330 495 2)" -f lavfi -i "$(stereo 550 825 2)" \
    -map 0:v -map 1:a -map 2:a -map 3:a \
    -c:v mpeg2video -q:v 10 -g 6 -bf 0 -flags +bitexact -c:a aac -b:a 64k \
    $same $ts three-sounds.ts

ls -l
