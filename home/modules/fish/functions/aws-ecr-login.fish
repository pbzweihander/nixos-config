function aws-ecr-login
    aws ecr get-login-password \
        | podman login \
        --username AWS \
        --password-stdin \
        "$(aws sts get-caller-identity --query Account --output text).dkr.ecr.$(aws configure get region).amazonaws.com"
end
