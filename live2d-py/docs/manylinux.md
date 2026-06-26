# Local manylinux build

## 1. Pull image

```bash
docker pull quay.io/pypa/manylinux_2_28_x86_64
```

## 2. Start persistent container (one time)

```bash
docker run -d --name ml -v $(pwd):/work -w /work \
  quay.io/pypa/manylinux_2_28_x86_64 sleep infinity

# Init: install deps (one time)
docker exec ml bash -c '
  yum install -y mesa-libGL-devel gcc-c++ libasan
  /opt/python/cp310-cp310/bin/pip install cmake
'
```

## 3. Build wheels

```bash
docker exec ml bash -c '
  set -e
  export PATH="/opt/python/cp310-cp310/bin:$PATH"
  export CC=/usr/bin/gcc CXX=/usr/bin/g++

  rm -rf build/ dist/

  for ver in 12; do
    PYBIN="/opt/python/cp3${ver}-cp3${ver}/bin"
    [ -x "$PYBIN/python" ] || continue
    echo "=== $PYBIN/python ==="
    "$PYBIN/pip" install "setuptools<82" wheel
    "$PYBIN/python" setup.py bdist_wheel
  done

  for whl in dist/*.whl; do
    auditwheel addtag "$whl" --plat manylinux_2_28_x86_64 -w dist/
  done

  chown -R $(stat -c "%u:%g" /work) /work/dist /work/build

  echo "=== Done ==="
  ls dist/
'
```

## 4. Verify tags

```bash
pip install auditwheel
for whl in dist/*.whl; do
  auditwheel show "$whl"
done
```

Expected: `manylinux_2_28_x86_64` platform tag.

## 5. Cleanup

```bash
docker rm -f ml
```
