# av1-fullgrain: the decoder (decoder/dav1d, a git submodule with
# github.com/divergentnn/dav1d-fullgrain) and the tools around it.
#
#   git clone --recursive https://github.com/divergentnn/av1-fullgrain.git
#   meson setup build && ninja -C build
#
# builds:
#   build/decoder/dav1d/src/libdav1d.*        the decoder library (dav1d 1.5.4 API)
#   build/decoder/dav1d/tools/dav1d           dav1d's command line decoder
#   build/tools/av1-grain-seed/av1-grain-seed the seed fixer (no dependencies)
#   build/tools/av1grain/av1grain             grain-free copies for the browser player
# Without the submodule (a ZIP download, say) only av1-grain-seed is built.
# Decoder options take the subproject's name: -Ddav1d:grain_mode_default=dual.
# The decoder can also be built on its own: meson setup build decoder/dav1d.
#
# Copyright © 2026, divergentnn (av1-fullgrain)
# SPDX-License-Identifier: BSD-2-Clause

project('av1-fullgrain', 'c',
    version : '1.0',
    license : 'BSD-2-Clause',
    meson_version : '>= 0.59.0',
    subproject_dir : 'decoder',
    default_options : ['buildtype=release', 'b_ndebug=if-release', 'warning_level=2'])

fs = import('fs')
cc = meson.get_compiler('c')
thread_dependency = dependency('threads')
libm_dependency = cc.find_library('m', required : false)

dav1d_proj = subproject('dav1d', required : false,
    default_options : ['enable_tests=false'])
if dav1d_proj.found()
    libdav1d = dav1d_proj.get_variable('libdav1d')
    dav1d_inc_dirs = dav1d_proj.get_variable('dav1d_inc_dirs')
    dav1d_input_objs = dav1d_proj.get_variable('dav1d_input_objs')
    dav1d_tests_dir = meson.current_source_dir() / 'decoder' / 'dav1d' / 'tests' / 'fullgrain'
    # what the decoder's own project adds for code that includes its headers
    dav1d_c_args = host_machine.system() in ['linux', 'gnu', 'emscripten'] ? ['-D_GNU_SOURCE'] : []
else
    message('decoder/dav1d is missing (git submodule update --init): building av1-grain-seed only')
endif

subdir('tools')
