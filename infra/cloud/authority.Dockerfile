# Build context: a tested API binary, immutable harnesses/ tree and the
# three scripts below. No downloaded CLI launcher or authenticated HOME belongs here.
# Supply an immutable API image digest built by Dockerfile.prebuilt.
ARG ZUSE_API_IMAGE
FROM ${ZUSE_API_IMAGE}
USER root
RUN apt-get update && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --chown=0:0 harnesses/ /opt/zuse/harnesses/
COPY auth-driver.py harness-release.py launch-authority.sh /usr/local/lib/zuse/
# A scoped staging candidate can omit channel pointers; production launch still
# requires its normal certified channel. Candidate mode rehashes exact binaries.
RUN chmod 0555 /usr/local/lib/zuse/auth-driver.py /usr/local/lib/zuse/harness-release.py /usr/local/lib/zuse/launch-authority.sh \
    && chmod -R a+rX /opt/zuse/harnesses && chmod -R a-w /opt/zuse/harnesses
USER 10001:10001
ENV ZUSE_HARNESS_ROOT=/opt/zuse/harnesses ZUSE_HARNESS_CHANNEL=staging
ENTRYPOINT ["/usr/local/lib/zuse/launch-authority.sh"]
CMD ["serve"]
