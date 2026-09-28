/*
 * doubao_ime.h —— doubao-ime 的 C ABI（同步）。
 *
 * 非官方实现，基于对豆包输入法私有接口的逆向分析。使用前请自行评估合规性。
 *
 * 约定：
 *   - 返回 int 的函数：0 成功；负值为错误（绝对值见 dbao_error_kind）。
 *   - 返回 char* 的输出参数由本库分配，须用 dbao_string_free 释放。
 *   - 字符串均为 UTF-8、NUL 结尾。
 *   - dbao_last_error() 返回当前线程最近一次错误信息（勿释放）。
 */
#ifndef DOUBAO_IME_H
#define DOUBAO_IME_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct DbaoClient DbaoClient;

/* 错误类别（错误码为其负值；-100 表示参数错误）。 */
typedef enum {
    DBAO_OK             = 0,
    DBAO_ERR_NETWORK    = 1,
    DBAO_ERR_PROTOCOL   = 2,
    DBAO_ERR_AUTH       = 3,
    DBAO_ERR_ASR        = 4,
    DBAO_ERR_API        = 5,
    DBAO_ERR_CONFIG     = 6,
    DBAO_ERR_INPUT      = 7,
    DBAO_ERR_CRYPTO     = 8,
    DBAO_ERR_ARG        = 100
} DbaoErrorKind;

/* 生命周期 */
DbaoClient *dbao_client_new(void);
void        dbao_client_free(DbaoClient *client);

/* 能力 */
int dbao_organize(const DbaoClient *client, const char *text, char **out);
int dbao_recognize_file(const DbaoClient *client, const char *path, int compat, char **out);
int dbao_recognize_pcm(const DbaoClient *client, const unsigned char *pcm, size_t len, int compat, char **out);

/* 辅助 */
const char *dbao_last_error(void);
void        dbao_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* DOUBAO_IME_H */
