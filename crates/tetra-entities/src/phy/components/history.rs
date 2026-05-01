/// Keep a history of past N samples of type T
pub struct History<T: Copy, const N: usize> {
    buffer: [T; N],
    /// Index where latest sample has been written
    index: usize,
}

impl<T: Copy, const N: usize> History<T, N> {
    pub fn new(initial_values: T) -> Self {
        Self {
            buffer: [initial_values; N],
            index: 0,
        }
    }

    /// Write a sample to buffer
    pub fn write(&mut self, sample: T) {
        self.index = (self.index + 1) % N;
        self.buffer[self.index] = sample;
    }

    /// Get a sample with a given delay
    pub fn delayed(&self, delay: usize) -> T {
        assert!(delay < N);
        self.buffer[(self.index + N - delay) % N]
    }
}
