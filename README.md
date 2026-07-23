# io_uring_benchmark

`TRANSPORT` 환경변수로 `io_uring` 또는 `epoll` 모드를 선택해서 같은 HTTP/file 작업을 비교하는 Rust 서버입니다.

`GET /work` 또는 `POST /work` 요청 하나마다:

1. `data/source.txt`에서 최대 4 KiB를 읽습니다.
2. 읽은 내용 뒤에 현재 UTC 시간을 추가합니다.
3. `data/output/<UUID>.txt`에 씁니다.
4. 본문 없는 `204 No Content`를 반환합니다.

모드별 차이:

- `TRANSPORT=io_uring`: `tokio-uring`으로 TCP accept/read/write와 파일 read/write를 처리합니다.
- `TRANSPORT=epoll`: Tokio 런타임으로 TCP 요청을 처리합니다. Linux에서는 네트워크 reactor가 epoll 기반입니다. 일반 파일은 epoll 대상이 아니므로 파일 read/write는 `tokio::fs` 경로를 사용합니다.

## 요구 사항

- Linux ARM64/aarch64 또는 x86_64
- Linux kernel 5.10+
- Rust stable

macOS에서는 io_uring가 없으므로 `io_uring` 모드는 실행할 수 없습니다.

## 실행

```bash
uname -m
uname -r

cargo build --release

# tokio-uring + io_uring, 기본값
TRANSPORT=io_uring ./target/release/io_uring_benchmark

# Tokio + epoll 네트워크
TRANSPORT=epoll cargo run --release
```

기본 주소는 `0.0.0.0:8080`입니다.

```bash
curl -i http://127.0.0.1:8080/work
ls -lh data/output | tail
```

주소 변경:

```bash
BIND_ADDRESS=0.0.0.0:9000 ./target/release/io_uring_benchmark
```

헬스체크:

```bash
curl -i http://127.0.0.1:8080/health
```

## write 완료와 실제 디스크 반영

기본값은 `write` 완료 후 응답하며 `fsync`는 하지 않습니다.

실제 저장장치까지 동기화한 뒤 응답하도록 비교하려면:

```bash
SYNC_WRITES=1 ./target/release/io_uring_benchmark
```

이 옵션은 지연시간과 처리량을 크게 바꿀 수 있습니다.

## 컨테이너 이미지

서버 이미지와 k6 이미지 두 개를 빌드합니다.

```bash
docker build -t io_uring_benchmark-server:0.1.0 -f Dockerfile .
docker build -t io_uring_benchmark-k6:0.1.0 -f Dockerfile.k6 .
```

원격 Kubernetes 클러스터에서 실행한다면 사용할 레지스트리 이름으로 태그를 바꾸고 push 한 뒤, `k8s/*.yaml`의 `image` 값을 같은 이름으로 맞추세요.

```bash
docker tag io_uring_benchmark-server:0.1.0 REGISTRY/io_uring_benchmark-server:0.1.0
docker tag io_uring_benchmark-k6:0.1.0 REGISTRY/io_uring_benchmark-k6:0.1.0
docker push REGISTRY/io_uring_benchmark-server:0.1.0
docker push REGISTRY/io_uring_benchmark-k6:0.1.0
```

## Kubernetes

서버와 k6 각각 Deployment + Service 매니페스트를 제공합니다.

```bash
kubectl apply -f k8s/namespace.yaml
kubectl apply -f k8s/server.yaml
kubectl apply -f k8s/k6.yaml
```

서버 Service:

```bash
kubectl get svc -n io-uring-benchmark io-uring-benchmark-server
```

k6는 Kubernetes 내부 DNS의 FQDN을 사용해 서버를 호출합니다.

```text
http://io-uring-benchmark-server.io-uring-benchmark.svc.cluster.local:8080
```

서버 모드 변경:

```bash
kubectl set env -n io-uring-benchmark deployment/io-uring-benchmark-server TRANSPORT=epoll
kubectl set env -n io-uring-benchmark deployment/io-uring-benchmark-server TRANSPORT=io_uring
```

k6 컨테이너는 테스트가 끝나도 Pod가 종료되지 않도록 `--linger`로 실행되며, `io-uring-benchmark-k6` Service는 k6 REST API 포트 `6565`를 가리킵니다.

k6 부하 패턴은 환경변수로 조절합니다. 기본값은 `30s` 동안 `10 RPS`로 워밍업한 뒤 `5m` 동안 `500 RPS`를 유지합니다.

```bash
kubectl set env -n io-uring-benchmark deployment/io-uring-benchmark-k6 \
  STEADY_DURATION=10m \
  STEADY_RPS=1000 \
  PRE_ALLOCATED_VUS=200 \
  MAX_VUS=2000
```

k6 결과 요약은 Pod 안의 `/tmp/summary/<timestamp>.html`, `/tmp/summary/<timestamp>.txt`에 저장됩니다. 이 경로는 `master0` node의 `/home/ubuntu/summary`에 `hostPath`로 마운트됩니다.

```bash
kubectl exec -n io-uring-benchmark deploy/io-uring-benchmark-k6 -- ls -lh /tmp/summary

K6_POD=$(kubectl get pod -n io-uring-benchmark -l app=io_uring_benchmark_k6 -o jsonpath='{.items[0].metadata.name}')
kubectl cp "io-uring-benchmark/${K6_POD}:/tmp/summary" ./summary
```

k6 summary 스크립트는 `https://raw.githubusercontent.com/benc-uk/k6-reporter/...`와 `https://jslib.k6.io/...`를 import합니다. 클러스터에서 Pod outbound 인터넷이 막혀 있다면 해당 모듈을 이미지에 vendoring 하거나 k6 archive로 묶어서 실행해야 합니다.

일부 Kubernetes seccomp 설정은 io_uring syscall을 막을 수 있어 `k8s/server.yaml`에 `seccompProfile: Unconfined`를 지정했습니다. 클러스터 정책에서 허용해야 합니다.

서버의 입력/출력 파일은 `master0` node의 hostPath에 저장됩니다.

- 입력 4 KiB 파일: node `/home/ubuntu/source-data/source.txt` → container `/app/data/source.txt`
- 생성 결과 디렉터리: node `/home/ubuntu/output` → container `/app/data/output`

`source.txt`가 없으면 서버 시작 시 기본 4 KiB 파일을 생성합니다. 직접 만들려면 worker node에서 아래처럼 실행하면 됩니다.

```bash
mkdir -p /home/ubuntu/source-data /home/ubuntu/output /home/ubuntu/summary
head -c 4096 /dev/zero | tr '\0' 'A' > /home/ubuntu/source-data/source.txt
wc -c /home/ubuntu/source-data/source.txt
```

## k6 부하 테스트

서버와 다른 머신에서 실행하는 것이 좋습니다.

```bash
k6 run -e BASE_URL=http://SERVER_IP:8080 scripts/load.js
```

기본 시나리오는 `30s @ 10 RPS` → `5m @ 500 RPS`입니다. `PRE_ALLOCATED_VUS`와 `MAX_VUS`는 목표 RPS를 만들기 위해 k6가 사용할 수 있는 VU 풀 크기입니다.

## 주의

- 요청마다 새 파일이 생기므로 디스크 용량과 inode를 확인하세요.
- 같은 4 KiB 원본 파일은 거의 항상 page cache에 남습니다.
- 진짜 저장장치 읽기 성능을 측정하려면 원본 파일 집합을 크게 만들거나 direct I/O 실험을 별도로 해야 합니다.
- 현재 서버는 벤치마크를 단순화하기 위해 요청 하나 처리 후 연결을 닫습니다.
