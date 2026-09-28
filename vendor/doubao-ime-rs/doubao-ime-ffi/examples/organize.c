/* 用 C 调用 doubao-ime 的最小示例。
 *
 * 编译：
 *   cargo build -p doubao-ime-ffi --release
 *   cc examples/organize.c -Iinclude -L../target/release -ldoubao_ime -o organize
 *   LD_LIBRARY_PATH=../target/release ./organize "嗯那个明天开会"
 */
#include "doubao_ime.h"
#include <stdio.h>

int main(int argc, char **argv) {
    const char *text = argc > 1 ? argv[1] : "嗯那个我想问一下明天几点开会";

    DbaoClient *client = dbao_client_new();
    if (!client) {
        fprintf(stderr, "创建客户端失败: %s\n", dbao_last_error());
        return 1;
    }

    char *out = NULL;
    int rc = dbao_organize(client, text, &out);
    if (rc == 0) {
        printf("整理结果: %s\n", out);
        dbao_string_free(out);
    } else {
        fprintf(stderr, "整理失败 (code=%d): %s\n", rc, dbao_last_error());
    }

    dbao_client_free(client);
    return rc == 0 ? 0 : 1;
}
