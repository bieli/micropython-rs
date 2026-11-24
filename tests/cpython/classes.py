class Point:
    dims = 2
    def __init__(self, x, y=0):
        self.x = x
        self.y = y
    def length2(self):
        return self.x * self.x + self.y * self.y
    def moved(self, dx, dy=0):
        return Point(self.x + dx, self.y + dy)
    def __str__(self):
        return 'Point(%d, %d)' % (self.x, self.y)
p = Point(3, 4)
print(p.x, p.y, p.length2(), p.dims, Point.dims, str(p), p)
q = p.moved(1, dy=1)
print(q, q.length2(), isinstance(q, Point), type(q) is Point, type(q).__name__, Point.__name__)
p.x += 10
p.label = 'P'
print(p.x, p.label, hasattr(p, 'label'), hasattr(p, 'nope'), getattr(p, 'nope', 'default'))
f = p.length2
print(f(), callable(f), callable(Point))
class Counter:
    count = 0
    def __init__(self):
        Counter.count += 1
        self.n = Counter.count
    @classmethod
    def made(cls):
        return cls.count
    @staticmethod
    def describe(word):
        return 'counter: ' + word
a = Counter()
b = Counter()
print(a.n, b.n, Counter.made(), a.made(), Counter.describe('hi'), b.describe('yo'), Counter.count)
class Animal:
    sound = '...'
    def __init__(self, name):
        self.name = name
    def speak(self):
        return self.name + ' says ' + self.sound
    def kind(self):
        return 'animal'
class Dog(Animal):
    sound = 'woof'
    def kind(self):
        return 'dog, an ' + super().kind()
class Puppy(Dog):
    def __init__(self, name, age):
        super().__init__(name)
        self.age = age
    def speak(self):
        return super().speak() + '!'
d = Dog('Rex')
pup = Puppy('Bit', 1)
print(d.speak(), d.kind(), pup.speak(), pup.kind(), pup.age)
print(isinstance(pup, Animal), isinstance(pup, Dog), isinstance(d, Puppy), isinstance(5, Animal))
class Repr:
    def __repr__(self):
        return 'Repr()'
r = Repr()
print(r, repr(r), str(r), [r, r], 'x' + str(r))
class Plain:
    pass
print(type(Plain()).__name__, Plain.__name__)
class AppError(Exception):
    pass
class SensorError(AppError):
    pass
try:
    raise SensorError('no reading', 36)
except AppError as e:
    print('caught', type(e).__name__, e.args, e, isinstance(e, Exception), isinstance(e, SensorError))
try:
    raise SensorError
except Exception as e:
    print('class raised', type(e).__name__)
try:
    raise AppError('plain')
except SensorError:
    print('wrong')
except AppError as e:
    print('right', e)
class NotFound(KeyError):
    pass
try:
    raise NotFound('k')
except LookupError as e:
    print('lookup', repr(e) if False else type(e).__name__)
class Managed:
    def __init__(self, tag):
        self.tag = tag
        self.log = []
    def __enter__(self):
        self.log.append('enter')
        return self
    def __exit__(self, kind, value, tb):
        self.log.append('exit')
        return False
m = Managed('m')
with m as entered:
    entered.log.append('body ' + entered.tag)
print(m.log)
def shout(fn):
    return lambda x: fn(x).upper()
@shout
def greet(name):
    return 'hello ' + name
print(greet('ann'))
class Registry:
    items = {}
    @classmethod
    def add(cls, key, value):
        cls.items[key] = value
        return cls
Registry.add('a', 1).add('b', 2)
print(Registry.items, sorted(Registry.items))
class Stack:
    def __init__(self):
        self.items = []
    def push(self, x):
        self.items.append(x)
        return self
    def pop(self):
        return self.items.pop()
    def __len__(self):
        return len(self.items)
s = Stack().push(1).push(2).push(3)
print(s.pop(), s.items, s.__len__())
objs = [Point(i, i) for i in range(3)]
print([o.length2() for o in objs], sorted(objs, key=lambda o: -o.x)[0].x)
try:
    p.missing
except AttributeError:
    print('no attribute')
try:
    Point()
except TypeError:
    print('missing argument')
