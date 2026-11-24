# How much of the ESP32's main stack Python code takes. stack_peak() returns
# the most stack bytes used since its previous call, so each line below shows
# the deepest point of one piece of code (including the REPL and eval()).

def peak(f, *args):
    stack_peak()
    try:
        f(*args)
    except RecursionError:
        pass
    return stack_peak()


def nothing():
    pass


def down(n):
    if n:
        down(n - 1)


def blocks(n):                     # four nested blocks per call
    if n >= 0:
        for i in [0]:
            while True:
                if True:
                    blocks(n - 1)
                    break


class Node:
    def __init__(self, depth):
        self.depth = depth

    def walk(self, a, *rest, **opts):
        with open('t_stack.txt', 'w') as fh:
            try:
                fh.write('%d' % self.depth)
                if self.depth:
                    return Node(self.depth - 1).walk(a, *rest, **opts)
                return f'{[((x + 1) * (2 - (3 + (4 * x)))) for x in range(3)]!r:>30}|{sorted(opts.items(), key=lambda kv: kv[0])}'
            except KeyError:
                pass


def node(depth):
    Node(depth).walk(1, 2, 3, k=1, j=2)


stack_peak()
x = 1
base = stack_peak()
print('statement at the top level:', base)
call = peak(nothing)
print('one call:', call)
one, seven = peak(down, 1), peak(down, 7)
print('calls, 2 and 8 deep:', one, seven, ' per call:', (seven - one) // 6)
print('recursion until RecursionError:', peak(down, 100))
b1, b2 = peak(blocks, 0), peak(blocks, 1)
print('4 blocks per call, 1 and 2 calls:', b1, b2, ' per call:', b2 - b1)
print('blocks until RecursionError:', peak(blocks, 100))
print('methods, with, try, **kwargs, f-strings, 6 deep:', peak(node, 5))
print('the same until RecursionError:', peak(node, 100))
import os
os.remove('t_stack.txt')
