/* Free function at file scope. */
int free_function() {
    int x = 1;
    return x;
}

/* Returns a greeting length for the given name. */
int documented(const char *name) {
    (void)name;
    return 0;
}

class Tracker {
public:
    Tracker() : value(0) {}
    void update() { value += 1; }

private:
    int value;
};

struct Point {
    int x;
    int y;
};

void use_point(Point p) {
    (void)p;
}
