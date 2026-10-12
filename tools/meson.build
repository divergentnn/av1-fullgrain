# av1-grain-seed: plain C11, no dependencies, built everywhere.
subdir('av1-grain-seed')

# av1grain uses the decoder's frame header parser (the fg_hook in
# decoder/dav1d/src/internal.h), so it needs the decoder submodule.
if dav1d_proj.found()
    subdir('av1grain')
endif
