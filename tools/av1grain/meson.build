# av1grain: a grain-free copy of an AV1 stream + the grain parameters it
# carried (the browser player draws the grain from them). It includes the
# decoder's src/internal.h for the parser hook, so it is built against the
# decoder in decoder/dav1d only.
#
# Copyright © 2026, divergentnn (av1-fullgrain)
# SPDX-License-Identifier: BSD-2-Clause

av1grain = executable('av1grain',
    files('av1grain.c'),
    link_with : [libdav1d],
    include_directories : [dav1d_inc_dirs],
    c_args : dav1d_c_args,
    dependencies : [thread_dependency],
    install : false,
)

# grain-free copy decodes like the original with grain off; copy + params ==
# original with grain, full and standard (sh tools/av1grain/strip.sh AV1GRAIN DIR)
sh = find_program('sh', required : false)
if sh.found() and host_machine.system() != 'windows'
    test('av1grain strip', sh,
        args : [files('strip.sh'), av1grain, dav1d_tests_dir],
        suite : 'av1grain',
        timeout : 300,
    )
endif
