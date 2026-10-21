# Our config lines, after Armada's overrides (the release, CONFIG_LOCALVERSION, comes from the PKGBUILD).
# No built-in initramfs (ours comes from the boot loader: thor-boot); the hostname; binder's devices left
# to binderfs (Waydroid makes them); DMA-BUF heaps (Android's allocators use /dev/dma_heap/system, which
# Waydroid passes into the container); the console turned (fbcon=rotate:1 on the command line: the panels
# are portrait), as ROCKNIX's config has it; the PMIC's clock built in, so the system clock is set from it
# at boot (RTC_HCTOSYS), as ROCKNIX's.
CONFIG_INITRAMFS_SOURCE=""
CONFIG_DEFAULT_HOSTNAME="thor"
CONFIG_ANDROID_BINDER_DEVICES=""
CONFIG_DMABUF_HEAPS=y
CONFIG_DMABUF_HEAPS_SYSTEM=y
CONFIG_FRAMEBUFFER_CONSOLE_ROTATION=y
CONFIG_RTC_DRV_PM8XXX=y
