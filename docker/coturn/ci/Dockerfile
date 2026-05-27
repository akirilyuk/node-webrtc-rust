FROM coturn/coturn:latest

COPY ci-turnserver.conf /etc/coturn/turnserver.conf

CMD ["turnserver", "-n", "-c", "/etc/coturn/turnserver.conf"]