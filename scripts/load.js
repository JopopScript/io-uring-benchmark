import http from "k6/http";
import { check } from "k6";
import { textSummary } from "https://jslib.k6.io/k6-summary/0.0.1/index.js";
import { htmlReport } from "https://raw.githubusercontent.com/benc-uk/k6-reporter/main/dist/bundle.js";

const warmupDuration = __ENV.WARMUP_DURATION || "30s";
const warmupRps = Number(__ENV.WARMUP_RPS || "10");
const steadyDuration = __ENV.STEADY_DURATION || "1s";
const steadyRps = Number(__ENV.STEADY_RPS || "1");
const preAllocatedVus = Number(__ENV.PRE_ALLOCATED_VUS || "100");
const maxVus = Number(__ENV.MAX_VUS || "1000");

export const options = {
  scenarios: {
    warmup: {
      executor: "constant-arrival-rate",
      rate: warmupRps,
      timeUnit: "1s",
      duration: warmupDuration,
      preAllocatedVUs: preAllocatedVus,
      maxVUs: maxVus,
      gracefulStop: "0s",
    },
    steady: {
      executor: "constant-arrival-rate",
      rate: steadyRps,
      timeUnit: "1s",
      duration: steadyDuration,
      startTime: warmupDuration,
      preAllocatedVUs: preAllocatedVus,
      maxVUs: maxVus,
      gracefulStop: "30s",
    },
  },
  thresholds: {
    http_req_failed: ["rate<0.01"],
    http_req_duration: ["p(95)<1000"],
  },
  noConnectionReuse: true,
};

const baseUrl = __ENV.BASE_URL || "http://127.0.0.1:8080";
const summaryDir = __ENV.SUMMARY_DIR || "/tmp/summary";

export default function () {
  const response = http.get(`${baseUrl}/work`, {
    tags: { endpoint: "work" },
  });

  check(response, {
    "status is 204": (r) => r.status === 204,
  });
}

export function handleSummary(data) {
  const timestamp = Date.now();
  const summaryText = textSummary(data, {
    indent: " ",
    enableColors: false,
  });

  return {
    [`${summaryDir}/${timestamp}.html`]: htmlReport(data),
    [`${summaryDir}/${timestamp}.txt`]: summaryText,
    stdout: summaryText,
  };
}
