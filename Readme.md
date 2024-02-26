
# Beam File

Samply.Beam.File allows bidirectional file transfer between two sites of the [Samply.Beam](https://github.com/samply/beam) network.

## Usage
In this description, we assume that you already have a running Samply.Beam installation consisting of a central Samply.Beam.Broker, a central Samply.Beam.CA and at least two local Samply.Proxies. To set this up, please see the [Samply.Beam Documentation](https://github.com/samply/beam/blob/main/README.md).

You can either build and run Samply.Beam.File as a local application, or use the docker image provided at the [Docker Hub](https://hub.docker.com/r/samply/beam-file).

> Note beam-file requires that beam is running with the sockets feature enabled. In order to start the demo version of beam with this feature run `./dev/beamdev --tag develop-sockets demo` inside the beam repository.

Configuration can either be done through command line arguments or env variables.
In the following example we will use be using the provided docker image and environment variables:

```yaml
services:
  ### Site A ###
  uploader:
    image: samply/beam-file
    container_name: uploader
    environment:
      - BEAM_ID=app1.proxy1.broker
      - BEAM_SECRET=App1Secret
      - BEAM_URL=http://proxy1:8081
      - BIND_ADDR=0.0.0.0:8085
      - API_KEY=SuperSecretApiKey
    ports:
      - 8085:8085
    networks:
      - dev_default

  ### Beam ###
  # In this example setup as mentioned above
  
  ### Site B ###
  downloader:
    image: samply/beam-file
    environment:
      - BEAM_ID=app1.proxy2.broker
      - BEAM_SECRET=App1Secret
      - BEAM_URL=http://proxy2:8082
      - BIND_ADDR=0.0.0.0:8086
      - CALLBACK=http://echo
      - API_KEY=OtherSuperSecretApiKey
    depends_on:
      - echo
    networks:
      - dev_default
      - default

  echo: # Simple http echo server for demonstration purposes
    image: mendhak/http-https-echo
    environment:
      - HTTP_PORT=80
    
networks:
  dev_default: # This is the default network that should have been created by starting beam
    external: true
```

After starting this compose process with `docker compose up` we should now be able to upload a file to the uploader at Site A and see its content being echoed by the http echo server located at Site B.
To do this we will be using curl:
```bash
curl -v -X POST -F "data=@Readme.md" -u ":SuperSecretApiKey"  http://localhost:8085/send/proxy2
```

If everything went well there should now be a logs message of the Readme emitted by the echo server.

`POST /send/:beam_proxy_name` is the only endpoint exposed this application. \
It requires basic auth with an empty username and the api key as the password. \
The body along with the following headers will be transmitted as is to the other proxy:
```
Content-Length
Content-Disposition
Content-Encoding
Content-Type
Metadata
```
Metadata is a custom header that can be used to send additional data about the file.
> Note: This header will not be encrypted by beam (only through regular tls). If the metadata contains confidential information it is recommended to send it as one part of the [multipart/form-data](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Type#content-type_in_html_forms) request.

## Configuration
As described by `--help`:
```
Options:
      --bind-addr <BIND_ADDR>      Address the server should bind to [env: BIND_ADDR=] [default: 0.0.0.0:8080]
      --beam-url <BEAM_URL>        Url of the local beam proxy which is required to have sockets enabled [env: BEAM_URL=] [default: http://beam-proxy:8081]
      --beam-secret <BEAM_SECRET>  Beam api key [env: BEAM_SECRET=]
      --api-key <API_KEY>          Api key required for uploading files [env: API_KEY=]
      --beam-id <BEAM_ID>          The app id of this application [env: BEAM_ID=]
      --callback <CALLBACK>        A url to an endpoint that will be called when we are receiving a new file [env: CALLBACK=]
  -h, --help                       Print help
```

The only interseting configuration that is not beam related is the callback url which may be specified by the site that wants to receive the uploaded data. It can be any endpoint and will be called by this application as a post request relaying the body and content related headers as uploaded by the other site.

For further documentation about the beam related options see the [beam repositroy](https://github.com/samply/beam).
