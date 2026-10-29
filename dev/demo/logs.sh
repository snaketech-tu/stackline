#!/bin/sh
# Prints realistic log lines for the Stackline demo stack. $KIND picks the format.
n=0
ts() { date -u +%Y-%m-%dT%H:%M:%S.000Z; }
while true; do
  n=$((n + 1))
  case "$KIND" in
    ecs)
      printf '{"@timestamp":"%s","log.level":"INFO","message":"GET /api/projects/%s 200 in %sms","log.logger":"com.example.web.RequestLog","process.thread.name":"http-nio-8080-exec-%s"}\n' "$(ts)" "$n" "$((n % 90 + 3))" "$((n % 8 + 1))"
      if [ $((n % 7)) -eq 0 ]; then
        printf '{"@timestamp":"%s","log.level":"ERROR","message":"Request processing failed for /api/projects/%s","log.logger":"o.a.c.c.C.[.[.[/].[dispatcherServlet]","error.type":"java.lang.IllegalStateException","error.message":"Project %s has no owner","error.stack_trace":"java.lang.IllegalStateException: Project %s has no owner\\n\\tat com.example.project.ProjectService.owner(ProjectService.java:142)\\n\\tat com.example.project.ProjectController.get(ProjectController.java:58)\\n\\tat java.base/jdk.internal.reflect.DirectMethodHandleAccessor.invoke(DirectMethodHandleAccessor.java:103)\\n"}\n' "$(ts)" "$n" "$n" "$n"
      fi
      if [ $((n % 11)) -eq 0 ]; then
        printf '{"@timestamp":"%s","log.level":"WARN","message":"Slow query took %sms","log.logger":"org.hibernate.SQL_SLOW"}\n' "$(ts)" "$((n * 37 % 4000 + 1000))"
      fi
      ;;
    logback)
      echo "$(date -u '+%Y-%m-%d %H:%M:%S.000')  INFO 1 --- [exec-$((n % 4))] c.a.shop.CheckoutController : Checkout preview for cart $n"
      if [ $((n % 5)) -eq 0 ]; then
        echo "$(date -u '+%Y-%m-%d %H:%M:%S.000') ERROR 1 --- [exec-$((n % 4))] c.a.shop.CheckoutController : Checkout failed for cart $n (replica $HOSTNAME)"
        echo "java.lang.NullPointerException: Cannot invoke \"Cart.getItems()\" because \"cart\" is null"
        echo "	at com.acme.shop.checkout.CheckoutService.total(CheckoutService.java:88)"
        echo "	at com.acme.shop.checkout.CheckoutController.preview(CheckoutController.java:41)"
        echo "	at org.springframework.web.servlet.FrameworkServlet.service(FrameworkServlet.java:885)"
        echo "Caused by: java.lang.IllegalArgumentException: cart id $n not found"
        echo "	... 47 common frames omitted"
      fi
      ;;
    python)
      echo "INFO:worker:picked up $((n % 9 + 1)) jobs from queue default" >&2
      if [ $((n % 6)) -eq 0 ]; then
        echo "ERROR:worker:job send-email#$n failed" >&2
        echo "Traceback (most recent call last):" >&2
        echo "  File \"/app/worker/queue.py\", line 57, in poll" >&2
        echo "    job = self.redis.blpop(self.queues, timeout=5)" >&2
        echo "  File \"/usr/local/lib/python3.12/site-packages/redis/connection.py\", line 353, in connect" >&2
        echo "    raise ConnectionError(self._error_message(e))" >&2
        echo "redis.exceptions.ConnectionError: Error 111 connecting to redis:6379. Connection refused." >&2
      fi
      if [ $((n % 8)) -eq 0 ]; then
        echo "ERROR:worker:job import-catalog#$n gave up after 5 attempts: GET https://catalog.example.com/api/v2/products?page=$((n % 40))&size=500&include=prices,stock,images,variants answered 503 Service Unavailable (upstream supplier-feed-eu-central-1 timed out after 30000ms, retry budget exhausted, next run scheduled by cron)" >&2
      fi
      ;;
    caddy)
      printf '{"level":"info","ts":%s.123,"logger":"http.log.access.log0","msg":"handled request","request":{"remote_ip":"10.0.0.%s","method":"GET","host":"shop.example.com","uri":"/api/projects/%s"},"duration":0.0%s,"status":%s}\n' "$(date +%s)" "$((n % 200))" "$n" "$((n % 90 + 10))" "$([ $((n % 13)) -eq 0 ] && echo 502 || echo 200)"
      if [ $((n % 13)) -eq 0 ]; then
        printf '{"level":"error","ts":%s.456,"logger":"http.log.error","msg":"dial tcp 172.18.0.4:8080: connect: connection refused","request":{"method":"GET","uri":"/api/projects/%s"},"status":502}\n' "$(date +%s)" "$n"
      fi
      ;;
  esac
  sleep "${DELAY:-1}"
done
