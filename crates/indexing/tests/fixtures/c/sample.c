/* Free function at file scope. */
int free_function(void) {
    int x = 1;
    return x;
}

/* Returns a greeting length for the given name. */
int documented(const char *name) {
    (void)name;
    return 0;
}

struct Tracker {
    int value;
};

void tracker_init(struct Tracker *t) {
    t->value = 0;
}

void tracker_update(struct Tracker *t) {
    t->value += 1;
}
