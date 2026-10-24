/* Stand-in for libgconf-2.so.4 (GNOME 2 config), which the client's libcef.so links against
 * but modern distributions no longer ship. CEF only asks it for GNOME proxy settings; every
 * call here reports "no client" / "no value". Built by setup-linux-client.sh. */
#include <stddef.h>

void *gconf_client_get_default(void) { return NULL; }
void gconf_client_add_dir(void *c, const char *d, int p, void **e) { if (e) *e = NULL; }
void gconf_client_remove_dir(void *c, const char *d, void **e) { if (e) *e = NULL; }
void *gconf_client_get(void *c, const char *k, void **e) { if (e) *e = NULL; return NULL; }
int gconf_client_get_int(void *c, const char *k, void **e) { if (e) *e = NULL; return 0; }
void *gconf_client_get_list(void *c, const char *k, int t, void **e) { if (e) *e = NULL; return NULL; }
char *gconf_client_get_string(void *c, const char *k, void **e) { if (e) *e = NULL; return NULL; }
unsigned gconf_client_notify_add(void *c, const char *n, void *f, void *d, void *df, void **e) {
  if (e) *e = NULL;
  return 0;
}
void gconf_client_notify_remove(void *c, unsigned id) {}
const char *gconf_entry_get_key(const void *entry) { return ""; }
void gconf_value_free(void *v) {}
int gconf_value_get_bool(const void *v) { return 0; }
