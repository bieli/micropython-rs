readings = [(i * 37) % 100 for i in range(8)]
print(sum(readings) // len(readings), max(readings), sorted(readings)[:3])
pins = {'led': 2, 'button': 0}
pins['buzzer'] = 25
for name, pin in pins.items():
    print(name, pin)
counts = {}
for word in 'the cat the dog'.split():
    counts[word] = counts.get(word, 0) + 1
print(counts)
x, y = 10, 20
x, y = y, x
print(x, y)
