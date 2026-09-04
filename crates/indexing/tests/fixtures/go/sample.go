package sample

// Free function at package scope.
func FreeFunction() int {
	x := 1
	return x
}

// Documented returns a greeting for the given name.
func Documented(name string) string {
	return "hello " + name
}

type Tracker struct {
	value int
}

func NewTracker() *Tracker {
	return &Tracker{value: 0}
}

func (t *Tracker) Update() {
	t.value++
}
