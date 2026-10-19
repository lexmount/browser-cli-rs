/* Kernel ABI comes exclusively from Apple's SDK; no copied socket structs.
 * Inspect only one recorded PID, never another process or an address range. */
#include <arpa/inet.h>
#include <libproc.h>
#include <netinet/in.h>
#include <stdint.h>
#include <stddef.h>
#include <sys/proc_info.h>
#include <sys/socket.h>
#include <time.h>

#define MAX_FDS 4096

static uint64_t monotonic_ns(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return 0;
    return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}

/* 1 = exact listener; 0 = absent; -1 = inspection uncertain; -2 = fd bound;
 * -3 = deadline. Every failure must be treated as unknown, never process gone.
 * Kernel calls have checks before and after, but are not forcibly interrupted. */
int lexmount_moli_has_listener(int pid, uint16_t port, uint64_t budget_ns) {
    uint64_t start = monotonic_ns();
    if (pid <= 1 || port == 0 || start == 0 || budget_ns == 0 ||
        budget_ns > UINT64_C(250000000)) return -1;
    uint64_t end = start + budget_ns;
    if (end < start) return -1;
#define CHECK_TIME() do { uint64_t n = monotonic_ns(); if (n == 0 || n >= end) return -3; } while (0)
    CHECK_TIME();
    int required = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, NULL, 0);
    CHECK_TIME();
    if (required <= 0 || required % (int)sizeof(struct proc_fdinfo) != 0) return -1;
    if (required > MAX_FDS * (int)sizeof(struct proc_fdinfo)) return -2;
    /* One extra slot detects a growing or truncated descriptor snapshot. */
    struct proc_fdinfo fds[MAX_FDS + 1];
    CHECK_TIME();
    int bytes = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, fds, (int)sizeof(fds));
    CHECK_TIME();
    if (bytes <= 0 || bytes % (int)sizeof(struct proc_fdinfo) != 0) return -1;
    if (bytes > MAX_FDS * (int)sizeof(struct proc_fdinfo)) return -2;
    int count = bytes / (int)sizeof(struct proc_fdinfo);
    for (int i = 0; i < count; i++) {
        CHECK_TIME();
        if (fds[i].proc_fdtype != PROX_FDTYPE_SOCKET) continue;
        struct socket_fdinfo socket;
        int read = proc_pidfdinfo(pid, fds[i].proc_fd, PROC_PIDFDSOCKETINFO,
                                 &socket, (int)sizeof(socket));
        CHECK_TIME();
        if (read != (int)sizeof(socket)) return -1;
        const struct socket_info *info = &socket.psi;
        if (info->soi_family != AF_INET || info->soi_type != SOCK_STREAM ||
            info->soi_protocol != IPPROTO_TCP || info->soi_kind != SOCKINFO_TCP)
            continue;
        const struct tcp_sockinfo *tcp = &info->soi_proto.pri_tcp;
        if (tcp->tcpsi_state == TSI_S_LISTEN &&
            (tcp->tcpsi_ini.insi_vflag & INI_IPV4) != 0 &&
            ntohs((uint16_t)tcp->tcpsi_ini.insi_lport) == port &&
            tcp->tcpsi_ini.insi_laddr.ina_46.i46a_addr4.s_addr == htonl(INADDR_LOOPBACK))
            return 1;
    }
    CHECK_TIME();
    return 0;
#undef CHECK_TIME
}
