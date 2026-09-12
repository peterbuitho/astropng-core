// Minimal smoke test for the C ABI: run a batch over an empty temp dir and
// print the resulting summary. Build & run with:
//   cc -Iinclude examples/smoke.c -Ltarget/debug -lastropng_core -lpthread -ldl -lm -o /tmp/smoke \
//     && LD_LIBRARY_PATH=target/debug /tmp/smoke
#include <stdio.h>
#include "astropng_core.h"

static void on_progress(const AstropngProgress *p, void *user_data) {
    (void)user_data;
    printf("progress %zu/%zu %s status=%d\n", p->index, p->total, p->rel_path, p->status);
}

int main(void) {
    printf("astropng-core version: %s\n", astropng_version());

    AstropngOptions opts = {0};
    opts.input_dir = "/tmp";
    opts.recursive = false;
    opts.png_only = false;

    AstropngSummary *summary = NULL;
    char *error = NULL;
    int rc = astropng_run(&opts, NULL, on_progress, NULL, &summary, &error);
    if (rc != 0) {
        printf("error (rc=%d): %s\n", rc, error);
        astropng_string_free(error);
        return 1;
    }

    printf("total=%zu converted=%u skipped=%u failed=%u cancelled=%d\n",
           summary->total, summary->converted, summary->skipped, summary->failed,
           summary->cancelled);
    astropng_summary_free(summary);
    return 0;
}
