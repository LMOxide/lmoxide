/* LD_PRELOAD fixes for the native Linux client (build 557666), loaded by play.sh.
 *
 * MCore::StringIDGenerator::GenerateIDForString (0x2825710) appends to a global name table and
 * grows it with realloc, without a lock. The client loads animations (EMotionFX motions and
 * motion sets) on its engine thread and on TBB worker threads at the same time, so two loads can
 * grow the table at once and corrupt the heap (crashes in HeapAllocator::Free under
 * GenerateIDForString, more often the more monster types a playfield has). This wraps the
 * function in a mutex, and reserves room up front so the table never moves under a reader.
 *
 * The function's first 12 bytes (push r13; push r12; mov r12,rsi; push rbp; mov rbp,rdi;
 * push rbx) are replaced by movabs rax,hook; jmp rax; a trampoline runs them and jumps back. */
#define _GNU_SOURCE
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define GENERATE_ID 0x2825710UL
#define RESERVE 0x2825990UL
/* entries reserved (16 bytes each); the table holds every motion and motion set name */
#define RESERVED_NAMES (1u << 16)

static const unsigned char PROLOGUE[12] = {0x41, 0x55, 0x41, 0x54, 0x49, 0x89, 0xf4, 0x55, 0x48, 0x89, 0xfd, 0x53};

static unsigned (*original)(void *, const char *);
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;

static unsigned generate_id(void *self, const char *name) {
    pthread_mutex_lock(&lock);
    /* capacity at +0xc (StringIDGenerator::Reserve) */
    if (((unsigned *)self)[3] < RESERVED_NAMES)
        ((void (*)(void *, unsigned))RESERVE)(self, RESERVED_NAMES);
    unsigned id = original(self, name);
    pthread_mutex_unlock(&lock);
    return id;
}

static void jump_to(unsigned char *at, uint64_t target) {
    at[0] = 0x48, at[1] = 0xb8; /* movabs rax, target */
    memcpy(at + 2, &target, 8);
    at[10] = 0xff, at[11] = 0xe0; /* jmp rax */
}

__attribute__((constructor)) static void install(void) {
    /* LD_PRELOAD reaches the client's helper processes too */
    char exe[4096];
    ssize_t n = readlink("/proc/self/exe", exe, sizeof exe - 1);
    if (n <= 0)
        return;
    exe[n] = 0;
    const char *base = strrchr(exe, '/');
    if (!base || strcmp(base + 1, "LMO") != 0)
        return;
    unsigned char *code = (unsigned char *)GENERATE_ID;
    if (memcmp(code, PROLOGUE, sizeof PROLOGUE) != 0) {
        fprintf(stderr, "lmo-fixes: unexpected client build, not patching\n");
        return;
    }
    unsigned char *tramp = mmap(NULL, 4096, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (tramp == MAP_FAILED)
        return;
    memcpy(tramp, PROLOGUE, sizeof PROLOGUE);
    jump_to(tramp + sizeof PROLOGUE, GENERATE_ID + sizeof PROLOGUE);
    original = (unsigned (*)(void *, const char *))tramp;
    long page = sysconf(_SC_PAGESIZE);
    void *start = (void *)(GENERATE_ID & ~(uintptr_t)(page - 1));
    if (mprotect(start, page, PROT_READ | PROT_WRITE | PROT_EXEC) != 0)
        return;
    jump_to(code, (uint64_t)(uintptr_t)generate_id);
    mprotect(start, page, PROT_READ | PROT_EXEC);
    fprintf(stderr, "lmo-fixes: StringIDGenerator locked\n");
}
