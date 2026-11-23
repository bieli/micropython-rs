def div(a, b):
    return a // b
try:
    div(1, 0)
except ZeroDivisionError:
    print('caught zero')
try:
    x = [1][5]
except (KeyError, IndexError) as e:
    print('lookup', type(e) is IndexError if False else 'ok')
try:
    raise ValueError('bad value')
except ValueError as e:
    print('value:', e, repr(e), e.args)
try:
    raise KeyError('k')
except LookupError as e:
    print('key:', e, repr(e))
try:
    raise OSError(2, 'no file')
except OSError as e:
    print('os:', e, e.errno, e.args)
try:
    {}['missing']
except Exception:
    print('exception base')
def f():
    try:
        return 'from try'
    finally:
        print('finally runs')
print(f())
def g():
    for i in range(3):
        try:
            if i == 1:
                continue
            if i == 2:
                break
            print('body', i)
        finally:
            print('fin', i)
    return 'g done'
print(g())
try:
    pass
except:
    print('never')
else:
    print('else runs')
finally:
    print('finally after else')
try:
    try:
        raise TypeError('inner')
    except TypeError:
        print('handled inner')
        raise
except TypeError as e:
    print('re-raised', e)
try:
    try:
        int('x')
    except ValueError:
        raise RuntimeError('converted')
    except RuntimeError:
        print('wrong: sibling handler')
except RuntimeError as e:
    print('outer got', e)
try:
    try:
        raise ValueError
    finally:
        print('cleanup')
except ValueError as e:
    print('propagated', repr(e))
try:
    assert 1 + 1 == 3, 'math is broken'
except AssertionError as e:
    print('assert:', e)
try:
    assert False
except AssertionError as e:
    print('assert no msg', repr(e))
try:
    undefined_name
except NameError:
    print('name error')
try:
    'a' + 1
except TypeError:
    print('type error')
try:
    import nosuchmodule
except ImportError:
    print('import error')
try:
    del nothing_here
except:
    pass
print('bare except ok')
count = 0
while True:
    try:
        count += 1
        if count < 3:
            raise ValueError(count)
        break
    except ValueError as e:
        print('retry', e)
print('count', count)
try:
    raise StopIteration
except StopIteration:
    print('stop')
e = 'kept'
try:
    raise ValueError('x')
except ValueError as e:
    pass
print('e deleted:', 'e' not in globals() if False else 'yes')
def nested():
    try:
        raise ValueError('deep')
    except KeyError:
        print('no')
try:
    nested()
except ValueError as err:
    print('from function:', err)
print(ValueError, isinstance if False else 'end')
