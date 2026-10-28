// Spike runner for the box design (docs/boxes.md): boots Calina's agent image
// under libkrun with the knobs the spike needs. Not product code.
//
//   krun-run --root-disk DEV:FS:OPTS --disk ID:PATH[:ro] [--mem MIB] [--cpus N]
//            [--vsock PORT:SOCK]         guest connects to PORT -> host unix SOCK
//            [--vsock-listen PORT:SOCK]  host connects to unix SOCK -> guest listens on PORT
//            [--env K=V]... [--workdir DIR] [--console FILE] [--log LEVEL]
//            -- CMD [ARGS...]
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int32_t krun_set_log_level(uint32_t level);
int32_t krun_create_ctx(void);
int32_t krun_set_vm_config(uint32_t ctx, uint8_t num_vcpus, uint32_t ram_mib);
int32_t krun_add_disk(uint32_t ctx, const char *block_id, const char *disk_path, bool read_only);
int32_t krun_set_root_disk_remount(uint32_t ctx, const char *device, const char *fstype, const char *options);
int32_t krun_disable_implicit_vsock(uint32_t ctx);
int32_t krun_add_vsock(uint32_t ctx, uint32_t tsi_features);
int32_t krun_add_vsock_port(uint32_t ctx, uint32_t port, const char *filepath);
int32_t krun_add_vsock_port2(uint32_t ctx, uint32_t port, const char *filepath, bool listen);
int32_t krun_set_workdir(uint32_t ctx, const char *workdir);
int32_t krun_set_console_output(uint32_t ctx, const char *filepath);
int32_t krun_set_exec(uint32_t ctx, const char *exec_path, const char *const argv[], const char *const envp[]);
int32_t krun_start_enter(uint32_t ctx);

#define MAX 16

static void die(const char *what, int err) {
  fprintf(stderr, "krun-run: %s failed: %d\n", what, err);
  exit(125);
}

static char *split_at_colon(char *s) {
  char *colon = strchr(s, ':');
  if (!colon) {
    fprintf(stderr, "krun-run: expected A:B, got %s\n", s);
    exit(125);
  }
  *colon = '\0';
  return colon + 1;
}

int main(int argc, char **argv) {
  char *root_disk = NULL, *disks[MAX], *vsocks[MAX], *listens[MAX];
  const char *envs[MAX + 3], *workdir = "/", *console = NULL;
  int ndisks = 0, nvsock = 0, nlisten = 0, nenv = 0, mem = 1024, cpus = 2, log = 0, i = 1;

  for (; i < argc; i++) {
    if (!strcmp(argv[i], "--")) { i++; break; }
    if (i + 1 >= argc) { fprintf(stderr, "krun-run: %s needs a value\n", argv[i]); return 125; }
    if (!strcmp(argv[i], "--root-disk")) root_disk = argv[++i];
    else if (!strcmp(argv[i], "--disk") && ndisks < MAX) disks[ndisks++] = argv[++i];
    else if (!strcmp(argv[i], "--vsock") && nvsock < MAX) vsocks[nvsock++] = argv[++i];
    else if (!strcmp(argv[i], "--vsock-listen") && nlisten < MAX) listens[nlisten++] = argv[++i];
    else if (!strcmp(argv[i], "--env") && nenv < MAX) envs[nenv++] = argv[++i];
    else if (!strcmp(argv[i], "--mem")) mem = atoi(argv[++i]);
    else if (!strcmp(argv[i], "--cpus")) cpus = atoi(argv[++i]);
    else if (!strcmp(argv[i], "--workdir")) workdir = argv[++i];
    else if (!strcmp(argv[i], "--console")) console = argv[++i];
    else if (!strcmp(argv[i], "--log")) log = atoi(argv[++i]);
    else { fprintf(stderr, "krun-run: unknown flag %s\n", argv[i]); return 125; }
  }
  if (!root_disk || i >= argc) {
    fprintf(stderr, "usage: see source\n");
    return 125;
  }
  envs[nenv++] = "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
  envs[nenv++] = "HOME=/root";
  envs[nenv++] = "TERM=xterm";
  envs[nenv] = NULL;

  int err;
  if (log && (err = krun_set_log_level((uint32_t)log))) die("krun_set_log_level", err);
  int ctx = krun_create_ctx();
  if (ctx < 0) die("krun_create_ctx", ctx);
  if ((err = krun_set_vm_config(ctx, (uint8_t)cpus, (uint32_t)mem))) die("krun_set_vm_config", err);
  for (int d = 0; d < ndisks; d++) {
    char *image = split_at_colon(disks[d]);
    char *mode = strrchr(image, ':');
    bool read_only = mode && !strcmp(mode, ":ro");
    if (mode) *mode = '\0';
    if ((err = krun_add_disk(ctx, disks[d], image, read_only))) die("krun_add_disk", err);
  }
  char *fstype = split_at_colon(root_disk);
  char *opts = split_at_colon(fstype);
  if ((err = krun_set_root_disk_remount(ctx, root_disk, fstype, opts))) die("krun_set_root_disk_remount", err);
  if ((err = krun_disable_implicit_vsock(ctx))) die("krun_disable_implicit_vsock", err);
  if ((err = krun_add_vsock(ctx, 0))) die("krun_add_vsock", err);
  for (int v = 0; v < nvsock; v++) {
    char *sock = split_at_colon(vsocks[v]);
    if ((err = krun_add_vsock_port(ctx, (uint32_t)atoi(vsocks[v]), sock))) die("krun_add_vsock_port", err);
  }
  for (int v = 0; v < nlisten; v++) {
    char *sock = split_at_colon(listens[v]);
    if ((err = krun_add_vsock_port2(ctx, (uint32_t)atoi(listens[v]), sock, true))) die("krun_add_vsock_port2", err);
  }
  if ((err = krun_set_workdir(ctx, workdir))) die("krun_set_workdir", err);
  if (console && (err = krun_set_console_output(ctx, console))) die("krun_set_console_output", err);
  if ((err = krun_set_exec(ctx, argv[i], (const char *const *)&argv[i + 1], envs))) die("krun_set_exec", err);

  err = krun_start_enter(ctx);
  die("krun_start_enter", err);
}
