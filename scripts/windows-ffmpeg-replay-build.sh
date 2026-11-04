#!/usr/bin/env bash
# Adaptation of BtbN FFmpeg-Builds build.sh at 5ece62897c650b863c1ddda7b6964845344aef2d.
set -euo pipefail
export TZ=UTC LC_ALL=C SOURCE_DATE_EPOCH=1785786727
mkdir -p /out/work /out/package
cd /out/work
git clone --no-checkout /inputs/ffmpeg.bundle ffmpeg
cd ffmpeg
[[ "$(git rev-parse refs/tags/n9.0)" == 5b9252abfe2fda0ccb71ed45cd7a954cc45d2b6f ]]
[[ "$(git rev-parse refs/tags/n9.0^{commit})" == d32b387f2b0a484599d4587d651891f0c63c4238 ]]
git checkout --detach d32b387f2b0a484599d4587d651891f0c63c4238
: "${FFBUILD_TARGET_FLAGS:?missing toolchain target flags}" "${FF_CONFIGURE:?missing toolchain configure flags}"
: "${CC:?missing compiler}" "${CXX:?missing compiler}" "${AR:?missing archiver}" "${RANLIB:?missing ranlib}" "${NM:?missing nm}"
case " $FF_CONFIGURE " in *' --enable-gpl '*) ;; *) echo 'toolchain lacks GPL configuration' >&2; exit 1;; esac
case " $FF_CONFIGURE " in *' --enable-version3 '*) ;; *) echo 'toolchain lacks version3 configuration' >&2; exit 1;; esac
case " $FF_CONFIGURE " in *' --enable-nonfree '*) echo 'nonfree configuration forbidden' >&2; exit 1;; esac
./configure --prefix=/out/work/prefix --pkg-config-flags=--static $FFBUILD_TARGET_FLAGS $FF_CONFIGURE \
    --extra-cflags="${FF_CFLAGS:-}" --extra-cxxflags="${FF_CXXFLAGS:-}" --extra-libs="${FF_LIBS:-}" \
    --extra-ldflags="${FF_LDFLAGS:-}" --extra-ldexeflags="${FF_LDEXEFLAGS:-}" \
    --cc="$CC" --cxx="$CXX" --ar="$AR" --ranlib="$RANLIB" --nm="$NM" \
    --extra-version="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y%m%d)"
for symbol in CONFIG_GPL CONFIG_VERSION3 CONFIG_LIBX264_ENCODER CONFIG_LIBX265_ENCODER CONFIG_LIBVPX_VP9_ENCODER CONFIG_LIBAOM_AV1_ENCODER CONFIG_LIBOPUS_ENCODER CONFIG_LIBVORBIS_ENCODER CONFIG_LIBMP3LAME_ENCODER CONFIG_LIBWEBP_ENCODER CONFIG_FFPLAY; do
    grep -qx "#define $symbol 1" config_components.h config.h || { echo "required FFmpeg capability absent: $symbol" >&2; exit 1; }
done
grep -qx '#define CONFIG_NONFREE 0' config.h || { echo 'nonfree configuration forbidden' >&2; exit 1; }
make -j"$(nproc)" V=1
make install install-doc
source /inputs/recipe/variants/win64-gpl.sh
package_variant /out/work/prefix /out/package
cp COPYING.GPLv3 /out/package/LICENSE.txt
cp ffbuild/config.log /out/config.log
