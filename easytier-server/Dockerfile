FROM --platform=$BUILDPLATFORM golang:1.24-alpine AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY *.go schema.sql ./
COPY web ./web
ARG TARGETOS
ARG TARGETARCH
RUN CGO_ENABLED=0 GOOS=$TARGETOS GOARCH=$TARGETARCH go build -trimpath -ldflags='-s -w' -o /easytier-server .
FROM alpine:3.22
RUN apk add --no-cache ca-certificates && adduser -D -u 10001 easytier
USER easytier
COPY --from=build /easytier-server /usr/local/bin/easytier-server
ENV LISTEN_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["easytier-server"]
